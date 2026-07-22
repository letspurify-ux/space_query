use fltk::{
    app,
    button::Button,
    draw,
    enums::{Align, CallbackTrigger, Event, Font, FrameType, Key, Shortcut},
    group::Group,
    input::Input,
    menu::MenuButton,
    prelude::*,
    table::{Table, TableContext},
    text::{TextBuffer, TextDisplay},
    window::Window,
};
use std::any::Any;
use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::query::result_messages;
use crate::db::{QueryExecutor, QueryResult};
use crate::ui::constants::*;
use crate::ui::font_settings::{
    configured_result_font_size, configured_result_profile, FontProfile,
};
use crate::ui::sql_editor::LazyFetchRequest;
use crate::ui::theme;
use crate::utils::arithmetic::safe_div;

fn byte_index_after_n_chars(s: &str, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    s.char_indices()
        .nth(n)
        .map(|(idx, _)| idx)
        .unwrap_or_else(|| s.len())
}

fn truncated_content_end(text: &str, max_chars: usize) -> Option<usize> {
    if max_chars == 0 {
        return if text.is_empty() { None } else { Some(0) };
    }

    if max_chars == 1 {
        if text.is_empty() {
            return None;
        }
        // Advance past the first character by scanning for the next char boundary.
        // If bytes remain after the first character, the text has more than one
        // character and must be truncated (showing only "…").
        let mut first_end = 1;
        while first_end < text.len() && !text.is_char_boundary(first_end) {
            first_end += 1;
        }
        return if first_end < text.len() {
            Some(0)
        } else {
            None
        };
    }

    let keep_chars = max_chars.saturating_sub(1);
    let keep_end = byte_index_after_n_chars(text, keep_chars);
    if keep_end >= text.len() {
        None
    } else {
        Some(keep_end)
    }
}

/// Stop computing column widths after this many rows (widths stabilize quickly)
const WIDTH_SAMPLE_ROWS: usize = 5000;
/// Limit stale row-position fallback scans so hit-testing stays responsive on huge datasets.
const MAX_HITTEST_ROW_BACKTRACK: i32 = 4096;
/// Limit stale column-position fallback scans for very wide result sets.
const MAX_HITTEST_COL_BACKTRACK: i32 = 512;
const HEADER_SORT_CLICK_MOVE_TOLERANCE_PX: u32 = 4;
const LAZY_FETCH_SCROLL_THRESHOLD_NUMERATOR: i64 = 1;
const LAZY_FETCH_SCROLL_THRESHOLD_DENOMINATOR: i64 = 1;
const LAZY_FETCH_SCROLLBAR_POLL_INTERVAL_SECONDS: f64 = 0.05;
const SORT_ASC_MARK: &str = "▲";
const SORT_DESC_MARK: &str = "▼";

pub type ResultGridSqlExecuteCallback =
    Arc<Mutex<Option<Box<dyn FnMut(String) -> Result<(), String>>>>>;
pub type LazyFetchCallback = Arc<Mutex<Option<Box<dyn FnMut(u64, LazyFetchRequest) -> bool>>>>;
pub type ResultTableContextActionCallback =
    Arc<Mutex<Option<Box<dyn FnMut(ResultTableContextAction)>>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResultTableContextAction {
    ExportCsv,
    Close,
    CloseAll,
}

enum LazyFetchPendingAction {
    HeaderSort(usize),
    CopyAll,
    SelectAll,
    MoveToEdge {
        edge: ResultTableEdge,
        selection: Option<(i32, i32, i32, i32)>,
    },
    ExtendSelectionToEdge {
        edge: ResultTableEdge,
        selection: Option<(i32, i32, i32, i32)>,
    },
    ExportCsv(Box<dyn FnMut(String, usize)>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResultTableEdge {
    Left,
    Right,
    Up,
    Down,
}

fn mutex_load_bool(flag: &Arc<Mutex<bool>>) -> bool {
    match flag.lock() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

fn mutex_store_bool(flag: &Arc<Mutex<bool>>, value: bool) {
    match flag.lock() {
        Ok(mut guard) => *guard = value,
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            *guard = value;
        }
    }
}

fn mutex_store_u64(value: &Arc<Mutex<u64>>, next: u64) {
    match value.lock() {
        Ok(mut guard) => *guard = next,
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            *guard = next;
        }
    }
}

fn mutex_fetch_add_u64(value: &Arc<Mutex<u64>>, delta: u64) -> u64 {
    match value.lock() {
        Ok(mut guard) => {
            let current = *guard;
            *guard = guard.saturating_add(delta);
            current
        }
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            let current = *guard;
            *guard = guard.saturating_add(delta);
            current
        }
    }
}

fn mutex_load_usize(value: &Arc<Mutex<usize>>) -> usize {
    match value.lock() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

fn mutex_store_usize(value: &Arc<Mutex<usize>>, next: usize) {
    match value.lock() {
        Ok(mut guard) => *guard = next,
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            *guard = next;
        }
    }
}

struct SharedFontSettings {
    normal_font: AtomicI32,
    bold_font: AtomicI32,
    italic_font: AtomicI32,
    font_size: AtomicU32,
}

impl SharedFontSettings {
    fn new(profile: FontProfile, size: u32) -> Self {
        Self {
            normal_font: AtomicI32::new(profile.normal.bits()),
            bold_font: AtomicI32::new(profile.bold.bits()),
            italic_font: AtomicI32::new(profile.italic.bits()),
            font_size: AtomicU32::new(crate::utils::AppConfig::clamp_font_size(size)),
        }
    }

    fn normal_font(&self) -> Font {
        usize::try_from(self.normal_font.load(Ordering::Relaxed))
            .ok()
            .map(Font::by_index)
            .unwrap_or(Font::Helvetica)
    }

    fn bold_font(&self) -> Font {
        usize::try_from(self.bold_font.load(Ordering::Relaxed))
            .ok()
            .map(Font::by_index)
            .unwrap_or(Font::HelveticaBold)
    }

    fn italic_font(&self) -> Font {
        usize::try_from(self.italic_font.load(Ordering::Relaxed))
            .ok()
            .map(Font::by_index)
            .unwrap_or(Font::HelveticaItalic)
    }

    fn profile(&self) -> FontProfile {
        FontProfile {
            name: "Shared",
            normal: self.normal_font(),
            bold: self.bold_font(),
            italic: self.italic_font(),
        }
    }

    fn size(&self) -> u32 {
        self.font_size.load(Ordering::Relaxed)
    }

    fn update(&self, profile: FontProfile, size: u32) {
        self.normal_font
            .store(profile.normal.bits(), Ordering::Relaxed);
        self.bold_font.store(profile.bold.bits(), Ordering::Relaxed);
        self.italic_font
            .store(profile.italic.bits(), Ordering::Relaxed);
        self.font_size.store(
            crate::utils::AppConfig::clamp_font_size(size),
            Ordering::Relaxed,
        );
    }
}

#[derive(Clone)]
pub struct ResultTableWidget {
    table: Table,
    headers: Arc<Mutex<Vec<String>>>,
    /// Buffer for pending rows during streaming
    pending_rows: Arc<Mutex<Vec<Vec<String>>>>,
    /// Pending column width updates
    pending_widths: Arc<Mutex<Vec<i32>>>,
    /// Last UI update time in epoch milliseconds
    last_flush_epoch_ms: Arc<Mutex<u64>>,
    /// The sole data store: full original data (non-truncated).
    /// draw_cell reads from here on demand — no data duplication.
    full_data: Arc<Mutex<Vec<Vec<String>>>>,
    /// Maximum displayed characters per cell; full text remains in full_data for copy/export.
    max_cell_display_chars: Arc<Mutex<usize>>,
    /// Lock-free mirror of max_cell_display_chars for hot draw path reads.
    max_cell_display_chars_draw: Arc<AtomicUsize>,
    /// How many rows have been sampled for column width calculation
    width_sampled_rows: Arc<Mutex<usize>>,
    font_settings: Arc<SharedFontSettings>,
    null_text: Arc<Mutex<String>>,
    source_sql: Arc<Mutex<String>>,
    execute_sql_callback: Arc<Mutex<Option<ResultGridSqlExecuteCallback>>>,
    edit_session: Arc<Mutex<Option<TableEditSession>>>,
    query_edit_backup: Arc<Mutex<Option<QueryEditBackupState>>>,
    pending_save_request: Arc<Mutex<bool>>,
    pending_save_sql_signature: Arc<Mutex<Option<String>>>,
    pending_save_request_tag: Arc<Mutex<Option<String>>>,
    pending_save_statement_signatures: Arc<Mutex<Vec<String>>>,
    next_save_request_id: Arc<Mutex<u64>>,
    hidden_auto_rowid_col: Arc<Mutex<Option<usize>>>,
    active_inline_edit: Arc<Mutex<Option<ActiveInlineEdit>>>,
    streaming_in_progress: Arc<Mutex<bool>>,
    lazy_fetch_session: Arc<Mutex<Option<u64>>>,
    lazy_fetch_callback: LazyFetchCallback,
    lazy_fetch_more_in_flight: Arc<Mutex<HashSet<u64>>>,
    pending_lazy_actions: Arc<Mutex<VecDeque<(u64, LazyFetchPendingAction)>>>,
    context_action_callback: ResultTableContextActionCallback,
    sort_state: Arc<Mutex<Option<ColumnSortState>>>,
    drag_state: Arc<Mutex<DragState>>,
}

#[derive(Default)]
struct DragState {
    is_dragging: bool,
    consume_background_pointer_sequence: bool,
    vertical_scrollbar_sequence: bool,
    vertical_scrollbar_polling: bool,
    vertical_scrollbar_fetch_requested: bool,
    header_sort_candidate_col: Option<i32>,
    header_sort_requires_double_click: bool,
    header_sort_start_x: i32,
    header_sort_start_y: i32,
    last_selection: Option<(i32, i32, i32, i32)>,
    last_col_position: Option<i32>,
    shift_click_base_selection: Option<(i32, i32, i32, i32)>,
    pending_shift_click_selection: Option<(i32, i32, i32, i32)>,
}

#[derive(Clone)]
enum EditRowState {
    Existing {
        rowid: String,
        explicit_null_cols: HashSet<usize>,
        dirty_cols: HashSet<usize>,
    },
    Inserted {
        explicit_null_cols: HashSet<usize>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CanonicalJoinClass {
    WordLike,
    Dot,
    Symbolic,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SortDirection {
    Ascending,
    Descending,
}

impl SortDirection {
    fn toggled(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct ColumnSortState {
    col_idx: usize,
    direction: SortDirection,
}

#[derive(Clone)]
struct TableEditSession {
    rowid_col: usize,
    table_name: String,
    null_text: String,
    editable_columns: Vec<(usize, String)>,
    original_rows_by_rowid: HashMap<String, Vec<String>>,
    original_row_order: Vec<String>,
    deleted_rowids: Vec<String>,
    row_states: Vec<EditRowState>,
}

#[derive(Clone)]
struct QueryEditBackupState {
    headers: Vec<String>,
    full_data: Vec<Vec<String>>,
    source_sql: String,
    edit_session: TableEditSession,
    sort_state: Option<ColumnSortState>,
}

struct EditModePreparation {
    table_name: String,
    rowid_col: usize,
    editable_columns: Vec<(usize, String)>,
}

#[derive(Clone)]
struct ActiveInlineEdit {
    row: usize,
    col: usize,
    input: Input,
}

impl ResultTableWidget {
    fn current_epoch_millis() -> u64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => {
                let millis = duration.as_millis();
                u64::try_from(millis).unwrap_or(u64::MAX)
            }
            Err(_) => 0,
        }
    }

    fn clear_pending_stream_buffers(&self) {
        self.pending_rows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.pending_widths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        mutex_store_usize(&self.width_sampled_rows, 0);
        mutex_store_u64(&self.last_flush_epoch_ms, Self::current_epoch_millis());
    }

    fn row_state_explicit_null_cols(row_state: &EditRowState) -> &HashSet<usize> {
        match row_state {
            EditRowState::Existing {
                explicit_null_cols, ..
            }
            | EditRowState::Inserted { explicit_null_cols } => explicit_null_cols,
        }
    }

    fn row_state_explicit_null_cols_mut(row_state: &mut EditRowState) -> &mut HashSet<usize> {
        match row_state {
            EditRowState::Existing {
                explicit_null_cols, ..
            }
            | EditRowState::Inserted { explicit_null_cols } => explicit_null_cols,
        }
    }

    fn row_state_dirty_cols_mut(row_state: &mut EditRowState) -> Option<&mut HashSet<usize>> {
        match row_state {
            EditRowState::Existing { dirty_cols, .. } => Some(dirty_cols),
            EditRowState::Inserted { .. } => None,
        }
    }

    fn sync_existing_row_dirty_cell(
        session: &mut TableEditSession,
        row_idx: usize,
        col_idx: usize,
        current_value: &str,
    ) {
        let is_dirty = session
            .row_states
            .get(row_idx)
            .and_then(|row_state| match row_state {
                EditRowState::Existing { rowid, .. } => session
                    .original_rows_by_rowid
                    .get(rowid)
                    .map(|original_row| {
                        let original_value = original_row
                            .get(col_idx)
                            .map(|value| value.as_str())
                            .unwrap_or("");
                        current_value != original_value
                    }),
                EditRowState::Inserted { .. } => Some(false),
            })
            .unwrap_or(false);

        if let Some(row_state) = session.row_states.get_mut(row_idx) {
            let Some(dirty_cols) = Self::row_state_dirty_cols_mut(row_state) else {
                return;
            };
            if is_dirty {
                dirty_cols.insert(col_idx);
            } else {
                dirty_cols.remove(&col_idx);
            }
        }
    }

    fn row_cell_is_explicit_null(
        session: &TableEditSession,
        row_idx: usize,
        col_idx: usize,
    ) -> bool {
        session
            .row_states
            .get(row_idx)
            .map(Self::row_state_explicit_null_cols)
            .map(|cols| cols.contains(&col_idx))
            .unwrap_or(false)
    }

    fn is_editable_column(session: &TableEditSession, col_idx: usize) -> bool {
        session
            .editable_columns
            .binary_search_by_key(&col_idx, |(editable_col, _)| *editable_col)
            .is_ok()
    }

    fn set_row_cell_explicit_null(
        session: &mut TableEditSession,
        row_idx: usize,
        col_idx: usize,
        is_explicit_null: bool,
    ) -> bool {
        let Some(row_state) = session.row_states.get_mut(row_idx) else {
            return false;
        };
        let cols = Self::row_state_explicit_null_cols_mut(row_state);
        if is_explicit_null {
            cols.insert(col_idx)
        } else {
            cols.remove(&col_idx)
        }
    }

    fn input_matches_null_text(input: &str, null_text: &str) -> bool {
        let marker = null_text.trim();
        if marker.is_empty() {
            return input.is_empty();
        }
        if marker.eq_ignore_ascii_case("NULL") {
            input.eq_ignore_ascii_case("NULL")
        } else {
            input == marker
        }
    }

    fn input_maps_to_explicit_null(row_state: &EditRowState, input: &str, null_text: &str) -> bool {
        let trimmed = input.trim();
        if Self::input_matches_null_text(trimmed, null_text) {
            return true;
        }
        if let Some(expr) = trimmed.strip_prefix('=') {
            return expr.trim().eq_ignore_ascii_case("NULL");
        }
        if input.is_empty() {
            return matches!(row_state, EditRowState::Existing { .. });
        }
        false
    }

    fn value_represents_null(value: &str, null_text: &str) -> bool {
        let trimmed = value.trim();
        value.is_empty()
            || trimmed.eq_ignore_ascii_case("NULL")
            || Self::input_matches_null_text(trimmed, null_text)
    }

    fn cell_edit_state(
        session: &TableEditSession,
        row_idx: usize,
        col_idx: usize,
        current_row: &[String],
    ) -> (bool, bool, bool) {
        if col_idx == session.rowid_col || !Self::is_editable_column(session, col_idx) {
            return (false, false, false);
        }

        let Some(row_state) = session.row_states.get(row_idx) else {
            return (false, false, false);
        };

        match row_state {
            EditRowState::Existing {
                rowid,
                explicit_null_cols,
                dirty_cols,
            } => {
                let is_explicit_null = explicit_null_cols.contains(&col_idx);
                let is_dirty = dirty_cols.contains(&col_idx);
                let Some(original_row) = session.original_rows_by_rowid.get(rowid) else {
                    return (is_dirty || is_explicit_null, is_explicit_null, false);
                };
                let current_value = current_row.get(col_idx).map(|v| v.as_str()).unwrap_or("");
                let original_value = original_row.get(col_idx).map(|v| v.as_str()).unwrap_or("");
                let is_original_null =
                    Self::value_represents_null(original_value, &session.null_text)
                        && Self::value_represents_null(current_value, &session.null_text);
                let is_modified = current_value != original_value || is_explicit_null || is_dirty;
                (is_modified, is_explicit_null, is_original_null)
            }
            EditRowState::Inserted { explicit_null_cols } => {
                let is_explicit_null = explicit_null_cols.contains(&col_idx);
                let has_non_empty_value = current_row
                    .get(col_idx)
                    .map(|value| !value.is_empty())
                    .unwrap_or(false);
                (
                    is_explicit_null || has_non_empty_value,
                    is_explicit_null,
                    false,
                )
            }
        }
    }

    fn cell_edit_state_for_draw(
        session: &TableEditSession,
        row_idx: usize,
        col_idx: usize,
        current_row: &[String],
    ) -> (bool, bool, bool) {
        if col_idx == session.rowid_col || !Self::is_editable_column(session, col_idx) {
            return (false, false, false);
        }

        let Some(row_state) = session.row_states.get(row_idx) else {
            return (false, false, false);
        };

        match row_state {
            EditRowState::Existing {
                rowid,
                explicit_null_cols,
                dirty_cols,
            } => {
                let is_explicit_null = explicit_null_cols.contains(&col_idx);
                let is_dirty = dirty_cols.contains(&col_idx);
                let current_value = current_row.get(col_idx).map(|v| v.as_str()).unwrap_or("");
                if !is_dirty && !is_explicit_null {
                    let is_original_null =
                        Self::value_represents_null(current_value, &session.null_text);
                    return (false, false, is_original_null);
                }
                let Some(original_row) = session.original_rows_by_rowid.get(rowid) else {
                    return (is_dirty || is_explicit_null, is_explicit_null, false);
                };
                let original_value = original_row.get(col_idx).map(|v| v.as_str()).unwrap_or("");
                let is_original_null =
                    Self::value_represents_null(original_value, &session.null_text)
                        && Self::value_represents_null(current_value, &session.null_text);
                let is_modified = current_value != original_value || is_explicit_null || is_dirty;
                (is_modified, is_explicit_null, is_original_null)
            }
            EditRowState::Inserted { explicit_null_cols } => {
                let is_explicit_null = explicit_null_cols.contains(&col_idx);
                let has_non_empty_value = current_row
                    .get(col_idx)
                    .map(|value| !value.is_empty())
                    .unwrap_or(false);
                (
                    is_explicit_null || has_non_empty_value,
                    is_explicit_null,
                    false,
                )
            }
        }
    }

    #[allow(dead_code)]
    fn row_cell_is_original_null(
        session: &TableEditSession,
        row_idx: usize,
        col_idx: usize,
        current_row: &[String],
    ) -> bool {
        let (_, _, is_original_null) =
            Self::cell_edit_state(session, row_idx, col_idx, current_row);
        is_original_null
    }

    fn current_null_text(&self) -> String {
        self.null_text
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn current_sort_state(&self) -> Option<ColumnSortState> {
        *self
            .sort_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn clear_sort_state(&self) {
        *self
            .sort_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    fn sort_marker_for_column(
        sort_state: Option<ColumnSortState>,
        col_idx: usize,
    ) -> Option<&'static str> {
        let state = sort_state?;
        if state.col_idx != col_idx {
            return None;
        }
        match state.direction {
            SortDirection::Ascending => Some(SORT_ASC_MARK),
            SortDirection::Descending => Some(SORT_DESC_MARK),
        }
    }

    fn parse_sort_number(value: &str) -> Option<f64> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        let parsed = trimmed.parse::<f64>().ok()?;
        if !parsed.is_finite() {
            return None;
        }
        Some(parsed)
    }

    fn compare_row_values_for_sort(
        left: &[String],
        right: &[String],
        col_idx: usize,
    ) -> std::cmp::Ordering {
        let left_value = left.get(col_idx).map(|value| value.as_str()).unwrap_or("");
        let right_value = right.get(col_idx).map(|value| value.as_str()).unwrap_or("");
        let left_number = Self::parse_sort_number(left_value);
        let right_number = Self::parse_sort_number(right_value);
        match (left_number, right_number) {
            (Some(lhs), Some(rhs)) => lhs.partial_cmp(&rhs).unwrap_or(std::cmp::Ordering::Equal),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left_value.cmp(right_value),
        }
    }

    fn sort_row_entries(
        rows: &mut Vec<Vec<String>>,
        row_states: Option<&mut Vec<EditRowState>>,
        col_idx: usize,
        direction: SortDirection,
    ) -> bool {
        match row_states {
            Some(states) => {
                if states.len() != rows.len() {
                    return false;
                }
                let moved_rows = std::mem::take(rows);
                let moved_states = std::mem::take(states);
                let mut paired: Vec<(Vec<String>, EditRowState)> =
                    moved_rows.into_iter().zip(moved_states).collect();
                paired.sort_by(|(left, _), (right, _)| {
                    let ordering = Self::compare_row_values_for_sort(left, right, col_idx);
                    match direction {
                        SortDirection::Ascending => ordering,
                        SortDirection::Descending => ordering.reverse(),
                    }
                });
                rows.reserve(paired.len());
                states.reserve(paired.len());
                for (row, state) in paired {
                    rows.push(row);
                    states.push(state);
                }
                true
            }
            None => {
                rows.sort_by(|left, right| {
                    let ordering = Self::compare_row_values_for_sort(left, right, col_idx);
                    match direction {
                        SortDirection::Ascending => ordering,
                        SortDirection::Descending => ordering.reverse(),
                    }
                });
                true
            }
        }
    }

    fn apply_sort_to_table_data(
        full_data: &Arc<Mutex<Vec<Vec<String>>>>,
        edit_session: &Arc<Mutex<Option<TableEditSession>>>,
        col_idx: usize,
        direction: SortDirection,
    ) -> bool {
        let mut session_guard = edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut rows_guard = full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let row_states = session_guard
            .as_mut()
            .map(|session| &mut session.row_states);
        Self::sort_row_entries(&mut rows_guard, row_states, col_idx, direction)
    }

    fn next_sort_state(current: Option<ColumnSortState>, col_idx: usize) -> ColumnSortState {
        match current {
            Some(state) if state.col_idx == col_idx => ColumnSortState {
                col_idx,
                direction: state.direction.toggled(),
            },
            _ => ColumnSortState {
                col_idx,
                direction: SortDirection::Ascending,
            },
        }
    }

    fn set_query_edit_backup(&self, backup: Option<QueryEditBackupState>) {
        *self
            .query_edit_backup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = backup;
    }

    fn restore_query_edit_backup(&mut self) -> bool {
        let backup = self
            .query_edit_backup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(backup) = backup else {
            return false;
        };

        *self
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = backup.headers;
        *self
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = backup.full_data;
        *self
            .source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = backup.source_sql;
        *self
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(backup.edit_session);
        *self
            .sort_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = backup.sort_state;

        let row_count = self
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len() as i32;
        let col_count = self
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len() as i32;
        self.set_table_rows_for_current_font(row_count);
        self.table.set_cols(col_count);
        self.recalculate_widths_for_current_font();
        self.refresh_auto_rowid_visibility();
        self.table.redraw();
        true
    }

    fn stage_query_edit_backup_from_current_state(&self, edit_session: TableEditSession) {
        let headers = self
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let full_data = self
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let source_sql = self
            .source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        self.set_query_edit_backup(Some(QueryEditBackupState {
            headers,
            full_data,
            source_sql,
            edit_session,
            sort_state: self.current_sort_state(),
        }));
    }

    fn clear_active_inline_edit_widget(active_inline_edit: &Arc<Mutex<Option<ActiveInlineEdit>>>) {
        let active_editor = active_inline_edit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(active_editor) = active_editor else {
            return;
        };

        let mut input = active_editor.input;
        if !input.was_deleted() {
            input.hide();
            if app::is_ui_thread() {
                Input::delete(input);
            }
        }
    }

    fn reposition_active_inline_editor(
        table: &Table,
        active_inline_edit: &Arc<Mutex<Option<ActiveInlineEdit>>>,
    ) {
        let guard = active_inline_edit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(active_editor) = guard.as_ref() else {
            return;
        };

        if active_editor.input.was_deleted() {
            drop(guard);
            *active_inline_edit
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            return;
        }

        let row = match i32::try_from(active_editor.row) {
            Ok(row) => row,
            Err(_) => return,
        };
        let col = match i32::try_from(active_editor.col) {
            Ok(col) => col,
            Err(_) => return,
        };

        let Some((x, y, w, h)) = table.find_cell(TableContext::Cell, row, col) else {
            return;
        };

        let input_x = x + 1;
        let input_y = y + 1;
        let input_w = (w - 2).max(24);
        let input_h = (h - 2).max(24);
        // Clone only the lightweight FLTK widget handle, then drop the lock
        // before calling resize/redraw to minimize lock hold time.
        let mut input = active_editor.input.clone();
        drop(guard);
        input.resize(input_x, input_y, input_w, input_h);
        input.redraw();
    }

    /// Returns the display column count for `text` using byte-level UTF-8 analysis.
    /// ASCII and 2-byte sequences count as 1 column; 3-byte (CJK etc.) and
    /// 4-byte (emoji etc.) sequences count as 2 columns.
    fn display_col_count(text: &str) -> usize {
        let bytes = text.as_bytes();
        let mut cols = 0usize;
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if b < 0x80 {
                cols += 1;
                i += 1;
            } else if b < 0xC0 {
                // Stray continuation byte — skip
                i += 1;
            } else if b < 0xE0 {
                // 2-byte sequence (U+0080..U+07FF): Latin, Greek, etc. — 1 col
                cols += 1;
                i += 2;
            } else if b < 0xF0 {
                // 3-byte sequence (U+0800..U+FFFF): includes CJK — 2 cols
                cols += 2;
                i += 3;
            } else {
                // 4-byte sequence (U+10000+): emoji, etc. — 2 cols
                cols += 2;
                i += 4;
            }
        }
        cols
    }

    /// Returns the display column count of the longest line in `text`,
    /// capped at `max_cell_display_chars`. Used for column width estimation
    /// so that multi-line cells are sized by their widest line, not total length.
    fn longest_line_char_count(text: &str, max_cell_display_chars: usize) -> usize {
        if max_cell_display_chars == 0 {
            return 0;
        }
        text.lines()
            .map(|line| Self::display_col_count(line).min(max_cell_display_chars))
            .max()
            .unwrap_or(0)
    }

    fn show_cell_text_dialog(value: &str, font_profile: FontProfile, font_size: u32) {
        let current_group = Group::try_current();
        Group::set_current(None::<&Group>);

        let mut dialog = Window::default()
            .with_size(760, 520)
            .with_label("Cell Value");
        crate::ui::center_on_main(&mut dialog);
        dialog.set_color(theme::panel_raised());
        dialog.make_modal(true);

        let mut display = TextDisplay::new(10, 10, 740, 460, None);
        display.set_color(theme::editor_bg());
        display.set_text_color(theme::text_primary());
        display.set_text_font(font_profile.normal);
        display.set_text_size(font_size as i32);
        display.wrap_mode(fltk::text::WrapMode::AtBounds, 0);
        theme::style_text_display_scrollbars(&display);

        let mut buf = TextBuffer::default();
        buf.set_text(value);
        display.set_buffer(buf);

        let mut close_btn = Button::new(335, 480, BUTTON_WIDTH, BUTTON_HEIGHT, "Close");
        close_btn.set_color(theme::button_secondary());
        close_btn.set_label_color(theme::text_primary());
        close_btn.set_frame(FrameType::RFlatBox);

        let mut dialog_for_close = dialog.clone();
        close_btn.set_callback(move |_| {
            dialog_for_close.hide();
            app::awake();
        });

        dialog.end();
        dialog.show();
        Group::set_current(current_group.as_ref());

        while dialog.shown() {
            app::wait();
        }

        // Explicitly destroy top-level dialog widgets to release native resources.
        Window::delete(dialog);
    }

    fn try_clone_cell_value(
        full_data: &Arc<Mutex<Vec<Vec<String>>>>,
        row: i32,
        col: i32,
    ) -> Option<String> {
        let data = full_data.try_lock().ok()?;
        data.get(row as usize)
            .and_then(|r| r.get(col as usize))
            .cloned()
    }

    fn visible_column_bounds(max_cols: usize, hidden_col: Option<usize>) -> Option<(usize, usize)> {
        if max_cols == 0 {
            return None;
        }

        let first_visible = (0..max_cols).find(|col| Some(*col) != hidden_col)?;
        let last_visible = (0..max_cols).rev().find(|col| Some(*col) != hidden_col)?;
        Some((first_visible, last_visible))
    }

    fn nearest_visible_column(
        max_cols: usize,
        preferred_col: usize,
        hidden_col: Option<usize>,
    ) -> Option<usize> {
        if max_cols == 0 {
            return None;
        }

        let max_col = max_cols.saturating_sub(1);
        let clamped_col = preferred_col.min(max_col);
        if Some(clamped_col) != hidden_col {
            return Some(clamped_col);
        }

        (0..clamped_col)
            .rev()
            .find(|col| Some(*col) != hidden_col)
            .or_else(|| {
                (clamped_col.saturating_add(1)..max_cols).find(|col| Some(*col) != hidden_col)
            })
    }

    /// Home/End mirror Ctrl+Left / Ctrl+Right; Ctrl+Home / Ctrl+End mirror Ctrl+Up / Ctrl+Down.
    fn arrow_key_for_home_end(key: Key, original_key: Key, ctrl_or_cmd: bool) -> Option<Key> {
        [key, original_key]
            .into_iter()
            .find_map(|event_key| match event_key {
                Key::Home if ctrl_or_cmd => Some(Key::Up),
                Key::Home => Some(Key::Left),
                Key::End if ctrl_or_cmd => Some(Key::Down),
                Key::End => Some(Key::Right),
                _ => None,
            })
    }

    fn edge_for_arrow_key(key: Key, original_key: Key) -> Option<ResultTableEdge> {
        [key, original_key]
            .into_iter()
            .find_map(|event_key| match event_key {
                Key::Left => Some(ResultTableEdge::Left),
                Key::Right => Some(ResultTableEdge::Right),
                Key::Up => Some(ResultTableEdge::Up),
                Key::Down => Some(ResultTableEdge::Down),
                _ => None,
            })
    }

    fn target_cell_for_edge(
        selection: (i32, i32, i32, i32),
        max_rows: usize,
        max_cols: usize,
        hidden_col: Option<usize>,
        edge: ResultTableEdge,
        fallback_row: usize,
        fallback_col: usize,
    ) -> Option<(usize, usize)> {
        if max_rows == 0 || max_cols == 0 {
            return None;
        }

        let (first_visible_col, last_visible_col) =
            Self::visible_column_bounds(max_cols, hidden_col)?;
        let (row, col) = Self::selection_bounds_excluding_hidden_column(
            selection, max_rows, max_cols, hidden_col,
        )
        .map(|(row_start, col_start, _, _)| (row_start, col_start))
        .unwrap_or_else(|| {
            (
                fallback_row.min(max_rows.saturating_sub(1)),
                Self::nearest_visible_column(max_cols, fallback_col, hidden_col)
                    .unwrap_or(first_visible_col),
            )
        });

        match edge {
            ResultTableEdge::Left => Some((row, first_visible_col)),
            ResultTableEdge::Right => Some((row, last_visible_col)),
            ResultTableEdge::Up => Some((0, col)),
            ResultTableEdge::Down => Some((max_rows.saturating_sub(1), col)),
        }
    }

    fn move_table_selection_to_edge_from_selection(
        table: &mut Table,
        selection: (i32, i32, i32, i32),
        edge: ResultTableEdge,
        hidden_col: Option<usize>,
        preserve_col_position: Option<i32>,
    ) -> bool {
        let rows = table.rows().max(0) as usize;
        let cols = table.cols().max(0) as usize;
        let fallback_row = table.row_position().max(0) as usize;
        let fallback_col = table.col_position().max(0) as usize;
        let Some((target_row, target_col)) = Self::target_cell_for_edge(
            selection,
            rows,
            cols,
            hidden_col,
            edge,
            fallback_row,
            fallback_col,
        ) else {
            return false;
        };

        let target_row = target_row as i32;
        let target_col = target_col as i32;
        table.set_selection(target_row, target_col, target_row, target_col);
        match edge {
            ResultTableEdge::Left | ResultTableEdge::Right => table.set_col_position(target_col),
            ResultTableEdge::Up | ResultTableEdge::Down => {
                table.set_row_position(Self::row_position_for_edge_target(
                    table,
                    target_row as usize,
                    edge,
                ));
                Self::restore_table_col_position(table, preserve_col_position)
            }
        }
        table.redraw();
        true
    }

    fn set_table_selection_bounds(
        table: &mut Table,
        bounds: (usize, usize, usize, usize),
        edge: ResultTableEdge,
        preserve_col_position: Option<i32>,
    ) -> (i32, i32, i32, i32) {
        let (row_start, col_start, row_end, col_end) = bounds;
        let (select_row_start, select_col_start, select_row_end, select_col_end) =
            Self::oriented_selection_for_edge(bounds, edge);
        table.set_selection(
            select_row_start,
            select_col_start,
            select_row_end,
            select_col_end,
        );
        match edge {
            ResultTableEdge::Left => table.set_col_position(col_start as i32),
            ResultTableEdge::Right => table.set_col_position(col_end as i32),
            ResultTableEdge::Up => {
                table.set_row_position(row_start as i32);
                Self::restore_table_col_position(table, preserve_col_position);
            }
            ResultTableEdge::Down => {
                table.set_row_position(Self::row_position_for_edge_target(table, row_end, edge));
                Self::restore_table_col_position(table, preserve_col_position);
            }
        }
        table.redraw();
        (
            select_row_start,
            select_col_start,
            select_row_end,
            select_col_end,
        )
    }

    fn row_position_for_edge_target(
        table: &Table,
        target_row: usize,
        edge: ResultTableEdge,
    ) -> i32 {
        let rows = table.rows().max(0) as usize;
        if rows == 0 {
            return 0;
        }
        let target_row = target_row.min(rows.saturating_sub(1));
        let visible_rows =
            Self::fully_visible_row_count_for_table(table, target_row as i32, rows as i32);
        Self::bounded_row_position_for_edge_target(target_row, rows, visible_rows, edge)
    }

    fn bounded_row_position_for_edge_target(
        target_row: usize,
        rows: usize,
        visible_rows: usize,
        edge: ResultTableEdge,
    ) -> i32 {
        if rows == 0 {
            return 0;
        }
        let target_row = target_row.min(rows.saturating_sub(1));
        let visible_rows = visible_rows.max(1).min(rows);
        let row_position = if edge == ResultTableEdge::Down {
            target_row.saturating_sub(visible_rows.saturating_sub(1))
        } else {
            target_row
        };
        row_position.min(rows.saturating_sub(1)) as i32
    }

    fn fully_visible_row_count_for_table(table: &Table, anchor_row: i32, rows: i32) -> usize {
        if rows <= 0 {
            return 1;
        }
        let anchor_row = anchor_row.max(0).min(rows.saturating_sub(1));
        let row_h = Self::uniform_row_height_near_viewport(table, anchor_row, rows)
            .unwrap_or_else(|| table.row_height(anchor_row));
        if row_h <= 0 {
            return 1;
        }
        let data_top = table.y().saturating_add(table.col_header_height());
        let data_bottom = Self::table_row_viewport_bottom(table);
        let visible_height = data_bottom.saturating_sub(data_top);
        safe_div(visible_height, row_h).max(1) as usize
    }

    fn table_row_viewport_bottom(table: &Table) -> i32 {
        let hsb = table.hscrollbar();
        if hsb.visible() {
            hsb.y()
        } else {
            table.y().saturating_add(table.h())
        }
    }

    fn oriented_selection_for_edge(
        bounds: (usize, usize, usize, usize),
        edge: ResultTableEdge,
    ) -> (i32, i32, i32, i32) {
        let (row_start, col_start, row_end, col_end) = (
            bounds.0 as i32,
            bounds.1 as i32,
            bounds.2 as i32,
            bounds.3 as i32,
        );
        match edge {
            ResultTableEdge::Left => (row_start, col_end, row_end, col_start),
            ResultTableEdge::Right => (row_start, col_start, row_end, col_end),
            ResultTableEdge::Up => (row_end, col_start, row_start, col_end),
            ResultTableEdge::Down => (row_start, col_start, row_end, col_end),
        }
    }

    fn extend_table_selection_to_edge_from_selection(
        table: &mut Table,
        selection: (i32, i32, i32, i32),
        edge: ResultTableEdge,
        hidden_col: Option<usize>,
        preserve_col_position: Option<i32>,
    ) -> Option<(i32, i32, i32, i32)> {
        let max_rows = table.rows().max(0) as usize;
        let max_cols = table.cols().max(0) as usize;
        let next_selection =
            Self::oriented_selection_to_edge(selection, max_rows, max_cols, hidden_col, edge)?;

        Self::set_table_oriented_selection(table, next_selection, edge, preserve_col_position);
        Some(next_selection)
    }

    fn set_table_oriented_selection(
        table: &mut Table,
        selection: (i32, i32, i32, i32),
        edge: ResultTableEdge,
        preserve_col_position: Option<i32>,
    ) {
        table.set_selection(selection.0, selection.1, selection.2, selection.3);
        let Some((row_start, col_start, row_end, col_end)) =
            Self::normalized_selection_bounds_with_limits(
                selection,
                table.rows().max(0) as usize,
                table.cols().max(0) as usize,
            )
        else {
            table.redraw();
            return;
        };

        match edge {
            ResultTableEdge::Left => table.set_col_position(col_start as i32),
            ResultTableEdge::Right => table.set_col_position(col_end as i32),
            ResultTableEdge::Up => {
                let target_row_position = row_start as i32;
                table.set_row_position(target_row_position);
                Self::restore_table_col_position(table, preserve_col_position);
                Self::schedule_table_row_position_restore(
                    table.clone(),
                    target_row_position,
                    preserve_col_position,
                );
            }
            ResultTableEdge::Down => {
                let target_row_position = Self::row_position_for_edge_target(table, row_end, edge);
                table.set_row_position(target_row_position);
                Self::restore_table_col_position(table, preserve_col_position);
                Self::schedule_table_row_position_restore(
                    table.clone(),
                    target_row_position,
                    preserve_col_position,
                );
            }
        }
        table.redraw();
    }

    fn schedule_table_row_position_restore(
        mut table: Table,
        target_row_position: i32,
        preserve_col_position: Option<i32>,
    ) {
        crate::ui::ui_timeout::schedule(0.0, move || {
            if table.was_deleted() {
                return;
            }
            Self::force_table_row_position(&mut table, target_row_position);
            Self::restore_table_col_position(&mut table, preserve_col_position);
            table.redraw();
        });
    }

    fn force_table_row_position(table: &mut Table, target_row_position: i32) {
        let rows = table.rows().max(0);
        if rows <= 0 {
            table.set_row_position(0);
            return;
        }

        let target_row_position = target_row_position.max(0).min(rows.saturating_sub(1));
        if table.row_position() == target_row_position && rows > 1 {
            let nudge_row = if target_row_position > 0 {
                target_row_position - 1
            } else {
                1
            };
            table.set_row_position(nudge_row);
        }
        table.set_row_position(target_row_position);
    }

    fn restore_table_col_position(table: &mut Table, col_position: Option<i32>) {
        let Some(col_position) = col_position else {
            return;
        };
        let cols = table.cols();
        if cols <= 0 {
            table.set_col_position(0);
            return;
        }
        table.set_col_position(col_position.max(0).min(cols.saturating_sub(1)));
    }

    fn preserved_col_position_for_edge(
        edge: ResultTableEdge,
        previous_col_position: Option<i32>,
    ) -> Option<i32> {
        match edge {
            ResultTableEdge::Up | ResultTableEdge::Down => previous_col_position,
            ResultTableEdge::Left | ResultTableEdge::Right => None,
        }
    }

    fn oriented_selection_with_limits(
        selection: (i32, i32, i32, i32),
        max_rows: usize,
        max_cols: usize,
    ) -> Option<(usize, usize, usize, usize)> {
        if max_rows == 0 || max_cols == 0 {
            return None;
        }
        let (anchor_row, anchor_col, focus_row, focus_col) = selection;
        if anchor_row < 0 || anchor_col < 0 || focus_row < 0 || focus_col < 0 {
            return None;
        }
        Self::normalized_selection_bounds_with_limits(selection, max_rows, max_cols)?;

        let max_row = max_rows.saturating_sub(1);
        let max_col = max_cols.saturating_sub(1);
        Some((
            (anchor_row as usize).min(max_row),
            (anchor_col as usize).min(max_col),
            (focus_row as usize).min(max_row),
            (focus_col as usize).min(max_col),
        ))
    }

    fn oriented_selection_to_edge(
        selection: (i32, i32, i32, i32),
        max_rows: usize,
        max_cols: usize,
        hidden_col: Option<usize>,
        edge: ResultTableEdge,
    ) -> Option<(i32, i32, i32, i32)> {
        let (anchor_row, raw_anchor_col, focus_row, raw_focus_col) =
            Self::oriented_selection_with_limits(selection, max_rows, max_cols)?;
        let (row_start, col_start, row_end, col_end) =
            Self::selection_bounds_excluding_hidden_column(
                selection, max_rows, max_cols, hidden_col,
            )?;
        let (first_visible_col, last_visible_col) =
            Self::visible_column_bounds(max_cols, hidden_col)?;

        let anchor_col = Self::nearest_visible_column(max_cols, raw_anchor_col, hidden_col)?;
        let focus_col = Self::nearest_visible_column(max_cols, raw_focus_col, hidden_col)?;
        let focus_col_for_vertical = if anchor_col <= focus_col {
            col_end
        } else {
            col_start
        };
        let focus_row_for_horizontal = if anchor_row <= focus_row {
            row_end
        } else {
            row_start
        };

        let (anchor_row, anchor_col, focus_row, focus_col) = match edge {
            ResultTableEdge::Left => (
                anchor_row,
                anchor_col,
                focus_row_for_horizontal,
                first_visible_col,
            ),
            ResultTableEdge::Right => (
                anchor_row,
                anchor_col,
                focus_row_for_horizontal,
                last_visible_col,
            ),
            ResultTableEdge::Up => (anchor_row, anchor_col, 0, focus_col_for_vertical),
            ResultTableEdge::Down => (
                anchor_row,
                anchor_col,
                max_rows.saturating_sub(1),
                focus_col_for_vertical,
            ),
        };

        Some((
            anchor_row as i32,
            anchor_col as i32,
            focus_row as i32,
            focus_col as i32,
        ))
    }

    #[cfg(all(test, not(target_os = "linux")))]
    fn extended_selection_bounds_to_edge(
        selection: (i32, i32, i32, i32),
        max_rows: usize,
        max_cols: usize,
        hidden_col: Option<usize>,
        edge: ResultTableEdge,
    ) -> Option<(usize, usize, usize, usize)> {
        let (mut row_start, mut col_start, mut row_end, mut col_end) =
            Self::selection_bounds_excluding_hidden_column(
                selection, max_rows, max_cols, hidden_col,
            )?;
        let (first_visible_col, last_visible_col) =
            Self::visible_column_bounds(max_cols, hidden_col)?;

        match edge {
            ResultTableEdge::Left => col_start = first_visible_col,
            ResultTableEdge::Right => col_end = last_visible_col,
            ResultTableEdge::Up => row_start = 0,
            ResultTableEdge::Down => row_end = max_rows.saturating_sub(1),
        }

        Some((row_start, col_start, row_end, col_end))
    }

    fn handle_ctrl_arrow_navigation(
        table: &mut Table,
        key: Key,
        original_key: Key,
        hidden_col: Option<usize>,
        previous_selection: Option<(i32, i32, i32, i32)>,
        previous_col_position: Option<i32>,
        lazy_fetch_session: &Arc<Mutex<Option<u64>>>,
        lazy_fetch_callback: &LazyFetchCallback,
        pending_lazy_actions: &Arc<Mutex<VecDeque<(u64, LazyFetchPendingAction)>>>,
    ) -> bool {
        let Some(edge) = Self::edge_for_arrow_key(key, original_key) else {
            return false;
        };

        let selection = previous_selection.unwrap_or_else(|| table.get_selection());
        let preserve_col_position =
            Self::preserved_col_position_for_edge(edge, previous_col_position);
        if edge == ResultTableEdge::Down
            && Self::queue_lazy_action_if_active(
                lazy_fetch_session,
                lazy_fetch_callback,
                pending_lazy_actions,
                LazyFetchPendingAction::MoveToEdge {
                    edge,
                    selection: Some(selection),
                },
            )
        {
            Self::move_table_selection_to_edge_from_selection(
                table,
                selection,
                edge,
                hidden_col,
                preserve_col_position,
            );
            return true;
        }

        Self::move_table_selection_to_edge_from_selection(
            table,
            selection,
            edge,
            hidden_col,
            preserve_col_position,
        );
        true
    }

    fn handle_ctrl_shift_arrow_selection(
        table: &mut Table,
        key: Key,
        original_key: Key,
        hidden_col: Option<usize>,
        previous_selection: Option<(i32, i32, i32, i32)>,
        previous_col_position: Option<i32>,
        drag_state: &Arc<Mutex<DragState>>,
        lazy_fetch_session: &Arc<Mutex<Option<u64>>>,
        lazy_fetch_callback: &LazyFetchCallback,
        pending_lazy_actions: &Arc<Mutex<VecDeque<(u64, LazyFetchPendingAction)>>>,
    ) -> bool {
        let Some(edge) = Self::edge_for_arrow_key(key, original_key) else {
            return false;
        };

        let selection = previous_selection.unwrap_or_else(|| table.get_selection());
        let preserve_col_position =
            Self::preserved_col_position_for_edge(edge, previous_col_position);
        if edge == ResultTableEdge::Down
            && Self::queue_lazy_action_if_active(
                lazy_fetch_session,
                lazy_fetch_callback,
                pending_lazy_actions,
                LazyFetchPendingAction::ExtendSelectionToEdge {
                    edge,
                    selection: Some(selection),
                },
            )
        {
            if let Some(next_selection) = Self::extend_table_selection_to_edge_from_selection(
                table,
                selection,
                edge,
                hidden_col,
                preserve_col_position,
            ) {
                Self::store_drag_selection_snapshot(drag_state, Some(next_selection));
            }
            return true;
        }

        let max_rows = table.rows().max(0) as usize;
        let max_cols = table.cols().max(0) as usize;
        if let Some(bounds) = Self::hidden_left_boundary_shift_restore_bounds(
            previous_selection,
            table.get_selection(),
            max_rows,
            max_cols,
            hidden_col,
            key,
            original_key,
        ) {
            let next_selection =
                Self::set_table_selection_bounds(table, bounds, edge, preserve_col_position);
            Self::store_drag_selection_snapshot(drag_state, Some(next_selection));
            return true;
        }

        if let Some(next_selection) = Self::extend_table_selection_to_edge_from_selection(
            table,
            selection,
            edge,
            hidden_col,
            preserve_col_position,
        ) {
            Self::store_drag_selection_snapshot(drag_state, Some(next_selection));
        }
        true
    }

    fn selection_bounds_excluding_hidden_column(
        selection: (i32, i32, i32, i32),
        max_rows: usize,
        max_cols: usize,
        hidden_col: Option<usize>,
    ) -> Option<(usize, usize, usize, usize)> {
        let (row_start, col_start, row_end, col_end) =
            Self::normalized_selection_bounds_with_limits(selection, max_rows, max_cols)?;
        let visible_cols = Self::visible_column_indices_in_range(col_start, col_end, hidden_col);
        if let (Some(first_visible), Some(last_visible)) =
            (visible_cols.first(), visible_cols.last())
        {
            return Some((row_start, *first_visible, row_end, *last_visible));
        }

        let fallback_col = Self::nearest_visible_column(max_cols, col_start, hidden_col)?;
        Some((row_start, fallback_col, row_end, fallback_col))
    }

    fn clamp_selection_to_visible_columns(table: &mut Table, hidden_col: Option<usize>) -> bool {
        let max_rows = table.rows().max(0) as usize;
        let max_cols = table.cols().max(0) as usize;
        let Some(current_bounds) = Self::normalized_selection_bounds_with_limits(
            table.get_selection(),
            max_rows,
            max_cols,
        ) else {
            return false;
        };
        let Some(next_bounds) = Self::selection_bounds_excluding_hidden_column(
            table.get_selection(),
            max_rows,
            max_cols,
            hidden_col,
        ) else {
            return false;
        };

        if current_bounds == next_bounds {
            return false;
        }

        table.set_selection(
            next_bounds.0 as i32,
            next_bounds.1 as i32,
            next_bounds.2 as i32,
            next_bounds.3 as i32,
        );
        true
    }

    fn should_consume_boundary_arrow_for_selection(
        selection: (i32, i32, i32, i32),
        max_rows: usize,
        max_cols: usize,
        hidden_col: Option<usize>,
        key: Key,
    ) -> bool {
        if max_rows == 0 || max_cols == 0 {
            return true;
        }

        let Some((row_start, col_start, row_end, col_end)) =
            Self::selection_bounds_excluding_hidden_column(
                selection, max_rows, max_cols, hidden_col,
            )
        else {
            return false;
        };
        let Some((first_visible_col, last_visible_col)) =
            Self::visible_column_bounds(max_cols, hidden_col)
        else {
            return true;
        };

        match key {
            Key::Left => col_start <= first_visible_col,
            Key::Right => col_end >= last_visible_col,
            Key::Up => row_start == 0,
            Key::Down => row_end >= max_rows.saturating_sub(1),
            _ => false,
        }
    }

    fn should_consume_boundary_arrow(table: &Table, key: Key, hidden_col: Option<usize>) -> bool {
        let rows = table.rows();
        let cols = table.cols();
        if rows <= 0 || cols <= 0 {
            return true;
        }

        Self::should_consume_boundary_arrow_for_selection(
            table.get_selection(),
            rows as usize,
            cols as usize,
            hidden_col,
            key,
        )
    }

    fn hidden_left_boundary_shift_restore_bounds(
        previous_selection: Option<(i32, i32, i32, i32)>,
        current_selection: (i32, i32, i32, i32),
        max_rows: usize,
        max_cols: usize,
        hidden_col: Option<usize>,
        key: Key,
        original_key: Key,
    ) -> Option<(usize, usize, usize, usize)> {
        let hidden_col = hidden_col?;
        if Self::edge_for_arrow_key(key, original_key)? != ResultTableEdge::Left {
            return None;
        }

        let (first_visible_col, _) = Self::visible_column_bounds(max_cols, Some(hidden_col))?;
        if hidden_col >= first_visible_col {
            return None;
        }

        let current_raw_bounds =
            Self::normalized_selection_bounds_with_limits(current_selection, max_rows, max_cols)?;
        if current_raw_bounds.1 > hidden_col {
            return None;
        }

        let previous_bounds = Self::selection_bounds_excluding_hidden_column(
            previous_selection?,
            max_rows,
            max_cols,
            Some(hidden_col),
        )?;
        if previous_bounds.1 > first_visible_col {
            return None;
        }

        let current_bounds = Self::selection_bounds_excluding_hidden_column(
            current_selection,
            max_rows,
            max_cols,
            Some(hidden_col),
        )?;
        if current_bounds.1 > first_visible_col {
            return None;
        }

        Some(previous_bounds)
    }

    fn current_table_selection_bounds(table: &Table) -> Option<(i32, i32, i32, i32)> {
        let (row_start, col_start, row_end, col_end) = Self::oriented_selection_with_limits(
            table.get_selection(),
            table.rows().max(0) as usize,
            table.cols().max(0) as usize,
        )?;
        Some((
            row_start as i32,
            col_start as i32,
            row_end as i32,
            col_end as i32,
        ))
    }

    fn sync_drag_selection_snapshot(table: &Table, drag_state: &Arc<Mutex<DragState>>) {
        let selection = Self::current_table_selection_bounds(table);
        let col_position = Some(table.col_position().max(0));
        let mut state = drag_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.last_selection = selection;
        state.last_col_position = col_position;
        state.shift_click_base_selection = None;
    }

    fn store_drag_selection_snapshot(
        drag_state: &Arc<Mutex<DragState>>,
        selection: Option<(i32, i32, i32, i32)>,
    ) {
        let mut state = drag_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.last_selection = selection;
        state.shift_click_base_selection = None;
    }

    fn store_oriented_selection_snapshot(
        table: &Table,
        drag_state: &Arc<Mutex<DragState>>,
        selection: (i32, i32, i32, i32),
    ) {
        let mut state = drag_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.last_selection = Some(selection);
        state.last_col_position = Some(table.col_position().max(0));
    }

    fn apply_table_metrics_for_current_font(&mut self) {
        let font_size = self.font_settings.size();
        self.table
            .set_col_header_height(Self::header_height_for_font(font_size));
    }

    fn row_height_for_font(size: u32) -> i32 {
        let size = crate::utils::AppConfig::clamp_font_size(size) as i32;
        (size + TABLE_CELL_PADDING * 2 + 4).max(TABLE_ROW_HEIGHT)
    }

    fn header_height_for_font(size: u32) -> i32 {
        let size = crate::utils::AppConfig::clamp_font_size(size) as i32;
        (size + TABLE_CELL_PADDING * 2 + 6).max(TABLE_COL_HEADER_HEIGHT)
    }

    fn set_table_rows_for_current_font(&mut self, row_count: i32) {
        let next_row_count = row_count.max(0);
        let current_rows = self.table.rows().max(0);
        if current_rows == next_row_count {
            return;
        }
        if next_row_count == 0 {
            self.table.set_rows(0);
            return;
        }

        let desired_height = Self::row_height_for_font(self.font_settings.size());
        if next_row_count < current_rows {
            self.table.set_rows(next_row_count);
            return;
        }

        if current_rows == 0 {
            self.table.set_rows(1);
            if self.table.row_height(0) != desired_height {
                self.table.set_row_height(0, desired_height);
            }
            if next_row_count != 1 {
                self.table.set_rows(next_row_count);
            }
            return;
        }

        let template_row = current_rows.saturating_sub(1);
        if self.table.row_height(template_row) == desired_height {
            self.table.set_rows(next_row_count);
            return;
        }

        // Existing rows keep their current height. Seed exactly one newly added
        // row with the new font metrics so later growth inherits the new height.
        let seed_row = current_rows;
        self.table.set_rows(seed_row.saturating_add(1));
        if self.table.row_height(seed_row) != desired_height {
            self.table.set_row_height(seed_row, desired_height);
        }
        if next_row_count != seed_row.saturating_add(1) {
            self.table.set_rows(next_row_count);
        }
    }

    fn min_col_width_for_font(size: u32) -> i32 {
        (size as i32 * 6).max(80)
    }

    fn max_col_width_for_font(size: u32) -> i32 {
        (size as i32 * 28).max(300)
    }

    fn estimate_text_width(text: &str, font_size: u32) -> i32 {
        let col_count = Self::display_col_count(text) as i32;
        let avg_char_px = safe_div((font_size as i32 * 62) + 99, 100);
        let raw = col_count.saturating_mul(avg_char_px) + TABLE_CELL_PADDING * 2 + 2;
        raw.clamp(
            Self::min_col_width_for_font(font_size),
            Self::max_col_width_for_font(font_size),
        )
    }

    fn estimate_display_width(text: &str, font_size: u32, max_cell_display_chars: usize) -> i32 {
        let display_chars = Self::longest_line_char_count(text, max_cell_display_chars);
        let avg_char_px = safe_div((font_size as i32 * 62) + 99, 100);
        let raw = display_chars as i32 * avg_char_px + TABLE_CELL_PADDING * 2 + 2;
        // Cap scales with the cell preview setting rather than a fixed font-based limit,
        // so users who raise the preview length get proportionally wider columns.
        // A hard ceiling of 2000 px prevents absurdly wide columns at large preview values.
        let setting_max =
            (max_cell_display_chars as i32 * avg_char_px + TABLE_CELL_PADDING * 2 + 2).min(2000);
        raw.clamp(Self::min_col_width_for_font(font_size), setting_max)
    }

    fn update_widths_with_row(
        widths: &mut Vec<i32>,
        row: &[String],
        font_size: u32,
        max_cell_display_chars: usize,
    ) {
        let min_width = Self::min_col_width_for_font(font_size);
        if row.len() > widths.len() {
            widths.resize(row.len(), min_width);
        }
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(Self::estimate_display_width(
                cell,
                font_size,
                max_cell_display_chars,
            ));
        }
    }

    fn compute_column_widths(
        headers: &[String],
        rows: &[Vec<String>],
        font_size: u32,
        max_cell_display_chars: usize,
    ) -> Vec<i32> {
        let mut widths: Vec<i32> = headers
            .iter()
            .map(|h| Self::estimate_text_width(h, font_size))
            .collect();

        let sample_count = rows.len().min(WIDTH_SAMPLE_ROWS);
        for row in rows.iter().take(sample_count) {
            Self::update_widths_with_row(&mut widths, row, font_size, max_cell_display_chars);
        }

        widths
    }

    fn apply_widths_to_table(&mut self, widths: &[i32]) {
        if widths.is_empty() {
            return;
        }
        if self.table.cols() < widths.len() as i32 {
            self.table.set_cols(widths.len() as i32);
        }
        for (i, width) in widths.iter().enumerate() {
            self.table.set_col_width(i as i32, *width);
        }
        self.apply_hidden_rowid_column_width();
    }

    fn recalculate_widths_for_current_font(&mut self) {
        let headers = self
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if headers.is_empty() {
            return;
        }

        let font_size = self.font_settings.size();
        let max_cell_display_chars = *self
            .max_cell_display_chars
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut widths: Vec<i32> = headers
            .iter()
            .map(|h| Self::estimate_text_width(h, font_size))
            .collect();

        let sampled_rows: Vec<Vec<String>> = {
            let full_data = self
                .full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            full_data.iter().take(WIDTH_SAMPLE_ROWS).cloned().collect()
        };

        for row in &sampled_rows {
            Self::update_widths_with_row(&mut widths, row, font_size, max_cell_display_chars);
        }

        if sampled_rows.len() < WIDTH_SAMPLE_ROWS {
            let remaining = WIDTH_SAMPLE_ROWS - sampled_rows.len();
            let pending_rows: Vec<Vec<String>> = {
                let pending = self
                    .pending_rows
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                pending.iter().take(remaining).cloned().collect()
            };
            for row in &pending_rows {
                Self::update_widths_with_row(&mut widths, row, font_size, max_cell_display_chars);
            }
        }

        *self
            .pending_widths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = widths.clone();
        self.apply_widths_to_table(&widths);
    }

    pub fn new() -> Self {
        Self::with_size(0, 0, 100, 100)
    }

    pub fn with_size(x: i32, y: i32, w: i32, h: i32) -> Self {
        let headers: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let full_data: Arc<Mutex<Vec<Vec<String>>>> = Arc::new(Mutex::new(Vec::new()));
        let font_settings = Arc::new(SharedFontSettings::new(
            configured_result_profile(),
            configured_result_font_size(),
        ));
        let max_cell_display_chars =
            Arc::new(Mutex::new(RESULT_CELL_MAX_DISPLAY_CHARS_DEFAULT as usize));
        let max_cell_display_chars_draw = Arc::new(AtomicUsize::new(
            RESULT_CELL_MAX_DISPLAY_CHARS_DEFAULT as usize,
        ));
        let null_text = Arc::new(Mutex::new("NULL".to_string()));
        let source_sql = Arc::new(Mutex::new(String::new()));
        let execute_sql_callback: Arc<Mutex<Option<ResultGridSqlExecuteCallback>>> =
            Arc::new(Mutex::new(None));
        let edit_session: Arc<Mutex<Option<TableEditSession>>> = Arc::new(Mutex::new(None));
        let query_edit_backup: Arc<Mutex<Option<QueryEditBackupState>>> =
            Arc::new(Mutex::new(None));
        let pending_save_request: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
        let pending_save_sql_signature: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let pending_save_request_tag: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let pending_save_statement_signatures: Arc<Mutex<Vec<String>>> =
            Arc::new(Mutex::new(Vec::new()));
        let next_save_request_id = Arc::new(Mutex::new(1_u64));
        let hidden_auto_rowid_col: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
        let active_inline_edit: Arc<Mutex<Option<ActiveInlineEdit>>> = Arc::new(Mutex::new(None));
        let streaming_in_progress: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
        let lazy_fetch_session: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
        let lazy_fetch_callback: LazyFetchCallback = Arc::new(Mutex::new(None));
        let lazy_fetch_more_in_flight: Arc<Mutex<HashSet<u64>>> =
            Arc::new(Mutex::new(HashSet::new()));
        let pending_lazy_actions: Arc<Mutex<VecDeque<(u64, LazyFetchPendingAction)>>> =
            Arc::new(Mutex::new(VecDeque::new()));
        let context_action_callback: ResultTableContextActionCallback = Arc::new(Mutex::new(None));
        let sort_state: Arc<Mutex<Option<ColumnSortState>>> = Arc::new(Mutex::new(None));
        let drag_state = Arc::new(Mutex::new(DragState::default()));

        let mut table = Table::new(x, y, w, h, None);

        // Apply dark theme colors
        table.set_color(theme::panel_bg());
        table.set_row_header(true);
        table.set_row_header_width(TABLE_ROW_HEADER_WIDTH);
        table.set_col_header(true);
        table.set_col_header_height(Self::header_height_for_font(configured_result_font_size()));
        table.set_row_height_all(Self::row_height_for_font(configured_result_font_size()));
        table.set_rows(0);
        table.set_cols(0);
        theme::style_table_scrollbars(&table);
        // Keep the default super_handle_first=true so Fl_Table's native handler
        // runs first. This lets the native Drag handler manage cell selection
        // (including auto-scroll) while our closure only tracks auxiliary state.
        table.end();

        // Capture theme colors once for draw_cell (avoids per-cell function calls)
        let cell_bg = theme::table_cell_bg();
        let cell_fg = theme::text_primary();
        let sel_bg = theme::selection_soft();
        let edited_cell_bg = fltk::enums::Color::from_rgb(74, 64, 26);
        let edited_sel_bg = fltk::enums::Color::from_rgb(96, 82, 40);
        let edited_cell_fg = fltk::enums::Color::from_rgb(255, 236, 183);
        let null_cell_bg = fltk::enums::Color::from_rgb(24, 88, 80);
        let null_sel_bg = fltk::enums::Color::from_rgb(33, 110, 100);
        let null_cell_fg = fltk::enums::Color::from_rgb(224, 255, 250);
        let null_edited_bg = fltk::enums::Color::from_rgb(50, 82, 56);
        let null_edited_sel_bg = fltk::enums::Color::from_rgb(66, 100, 72);
        let null_edited_fg = fltk::enums::Color::from_rgb(230, 255, 210);
        let header_bg = theme::table_header_bg();
        let header_fg = theme::text_primary();
        let border_color = theme::table_border();

        // Virtual rendering: draw_cell reads directly from full_data on demand.
        // Only visible cells are rendered — no per-cell data stored in the Table widget.
        let headers_for_draw = headers.clone();
        let full_data_for_draw = full_data.clone();
        let table_for_draw = table.clone();
        let font_settings_for_draw = font_settings.clone();
        let max_cell_display_chars_for_draw = max_cell_display_chars_draw.clone();
        let edit_session_for_draw = edit_session.clone();
        let sort_state_for_draw = sort_state.clone();

        table.draw_cell(move |_t, ctx, row, col, x, y, w, h| {
            let normal_font = font_settings_for_draw.normal_font();
            let bold_font = font_settings_for_draw.bold_font();
            let font_size = font_settings_for_draw.size() as i32;
            match ctx {
                TableContext::StartPage => {
                    draw::set_font(normal_font, font_size);
                }
                TableContext::ColHeader => {
                    draw::push_clip(x, y, w, h);
                    draw::draw_box(FrameType::FlatBox, x, y, w, h, header_bg);
                    draw::set_draw_color(header_fg);
                    draw::set_font(bold_font, font_size);
                    let sort_snapshot =
                        sort_state_for_draw.try_lock().ok().and_then(|guard| *guard);
                    if let Ok(hdrs) = headers_for_draw.try_lock() {
                        if let Some(text) = hdrs.get(col as usize) {
                            draw::draw_text2(
                                text,
                                x + TABLE_CELL_PADDING,
                                y,
                                w - TABLE_CELL_PADDING * 2,
                                h,
                                Align::Left,
                            );
                            if let Some(marker) =
                                Self::sort_marker_for_column(sort_snapshot, col as usize)
                            {
                                draw::draw_text2(
                                    marker,
                                    x + TABLE_CELL_PADDING,
                                    y,
                                    w - TABLE_CELL_PADDING * 2,
                                    h,
                                    Align::Right,
                                );
                            }
                        }
                    }
                    draw::set_draw_color(border_color);
                    draw::draw_line(x, y + h - 1, x + w, y + h - 1);
                    draw::pop_clip();
                }
                TableContext::RowHeader => {
                    draw::push_clip(x, y, w, h);
                    draw::draw_box(FrameType::FlatBox, x, y, w, h, header_bg);
                    draw::set_draw_color(header_fg);
                    draw::set_font(normal_font, font_size);
                    let text = (row + 1).to_string();
                    draw::draw_text2(&text, x, y, w - TABLE_CELL_PADDING, h, Align::Right);
                    draw::set_draw_color(border_color);
                    draw::draw_line(x + w - 1, y, x + w - 1, y + h);
                    draw::pop_clip();
                }
                TableContext::Cell => {
                    draw::push_clip(x, y, w, h);
                    let selected = table_for_draw.is_selected(row, col);
                    let max_chars = max_cell_display_chars_for_draw.load(Ordering::Relaxed);
                    let mut is_edited_cell = false;
                    let mut is_explicit_null_cell = false;
                    let mut is_original_null_cell = false;
                    let draw_cell_contents =
                        |cell_value: Option<&str>,
                         is_edited_cell: bool,
                         is_explicit_null_cell: bool,
                         is_original_null_cell: bool| {
                            let is_null_cell = is_explicit_null_cell || is_original_null_cell;
                            let (bg, fg) = if is_null_cell && is_edited_cell {
                                // Explicit null on a modified cell: use a distinct
                                // hybrid tint so the user can tell apart "originally
                                // null, untouched" from "explicitly set to null".
                                if selected {
                                    (null_edited_sel_bg, null_edited_fg)
                                } else {
                                    (null_edited_bg, null_edited_fg)
                                }
                            } else if is_null_cell {
                                if selected {
                                    (null_sel_bg, null_cell_fg)
                                } else {
                                    (null_cell_bg, null_cell_fg)
                                }
                            } else if is_edited_cell {
                                if selected {
                                    (edited_sel_bg, edited_cell_fg)
                                } else {
                                    (edited_cell_bg, edited_cell_fg)
                                }
                            } else if selected {
                                (sel_bg, cell_fg)
                            } else {
                                (cell_bg, cell_fg)
                            };
                            draw::draw_box(FrameType::FlatBox, x, y, w, h, bg);
                            draw::set_draw_color(fg);
                            draw::set_font(normal_font, font_size);

                            if let Some(cell_val) = cell_value {
                                if let Some(truncated_end) =
                                    truncated_content_end(cell_val, max_chars)
                                {
                                    if truncated_end > 0 {
                                        let visible = &cell_val[..truncated_end];
                                        draw::draw_text2(
                                            visible,
                                            x + TABLE_CELL_PADDING,
                                            y,
                                            w - TABLE_CELL_PADDING * 2,
                                            h,
                                            Align::Left,
                                        );
                                    }
                                    draw::draw_text2(
                                        "…",
                                        x + TABLE_CELL_PADDING,
                                        y,
                                        w - TABLE_CELL_PADDING * 2,
                                        h,
                                        Align::Right,
                                    );
                                } else {
                                    draw::draw_text2(
                                        cell_val,
                                        x + TABLE_CELL_PADDING,
                                        y,
                                        w - TABLE_CELL_PADDING * 2,
                                        h,
                                        Align::Left,
                                    );
                                }
                            }

                            draw::set_draw_color(border_color);
                            draw::draw_line(x, y + h - 1, x + w, y + h - 1);
                            draw::draw_line(x + w - 1, y, x + w - 1, y + h);
                        };

                    if let (Ok(row_idx), Ok(col_idx)) = (usize::try_from(row), usize::try_from(col))
                    {
                        if let Ok(data) = full_data_for_draw.try_lock() {
                            if let Some(row_data) = data.get(row_idx) {
                                if let Ok(session_guard) = edit_session_for_draw.try_lock() {
                                    if let Some(session) = session_guard.as_ref() {
                                        (
                                            is_edited_cell,
                                            is_explicit_null_cell,
                                            is_original_null_cell,
                                        ) = Self::cell_edit_state_for_draw(
                                            session, row_idx, col_idx, row_data,
                                        );
                                    }
                                }
                                draw_cell_contents(
                                    row_data.get(col_idx).map(|value| value.as_str()),
                                    is_edited_cell,
                                    is_explicit_null_cell,
                                    is_original_null_cell,
                                );
                                draw::pop_clip();
                                return;
                            }
                        }
                    }

                    draw_cell_contents(
                        None,
                        is_edited_cell,
                        is_explicit_null_cell,
                        is_original_null_cell,
                    );
                    draw::pop_clip();
                }
                _ => {}
            }
        });

        // Setup event handler for mouse selection and keyboard shortcuts
        let headers_for_handle = headers.clone();
        let drag_state_for_handle = drag_state.clone();

        let mut table_for_handle = table.clone();
        let full_data_for_handle = full_data.clone();
        let font_settings_for_handle = font_settings.clone();
        let source_sql_for_handle = source_sql.clone();
        let execute_sql_callback_for_handle = execute_sql_callback.clone();
        let edit_session_for_handle = edit_session.clone();
        let pending_save_request_for_handle = pending_save_request.clone();
        let hidden_auto_rowid_col_for_handle = hidden_auto_rowid_col.clone();
        let active_inline_edit_for_handle = active_inline_edit.clone();
        let active_inline_edit_for_resize = active_inline_edit.clone();
        let sort_state_for_handle = sort_state.clone();
        let streaming_in_progress_for_handle = streaming_in_progress.clone();
        let lazy_fetch_session_for_handle = lazy_fetch_session.clone();
        let lazy_fetch_callback_for_handle = lazy_fetch_callback.clone();
        let lazy_fetch_more_in_flight_for_handle = lazy_fetch_more_in_flight.clone();
        let pending_lazy_actions_for_handle = pending_lazy_actions.clone();
        let context_action_callback_for_handle = context_action_callback.clone();
        table.handle(move |_, ev| {
            if !table_for_handle.active() {
                return false;
            }
            match ev {
                Event::Push => {
                    let mouse_x = app::event_x();
                    let mouse_y = app::event_y();
                    if Self::is_mouse_on_vertical_table_scrollbar(
                        &table_for_handle,
                        mouse_x,
                        mouse_y,
                    ) {
                        let should_start_scrollbar_poll = {
                            let mut state = drag_state_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            state.vertical_scrollbar_sequence = true;
                            state.vertical_scrollbar_fetch_requested = false;
                            state.shift_click_base_selection = None;
                            state.pending_shift_click_selection = None;
                            if state.vertical_scrollbar_polling {
                                false
                            } else {
                                state.vertical_scrollbar_polling = true;
                                true
                            }
                        };
                        Self::request_lazy_fetch_more_for_scrollbar_sequence(
                            &table_for_handle,
                            &lazy_fetch_session_for_handle,
                            &lazy_fetch_callback_for_handle,
                            &lazy_fetch_more_in_flight_for_handle,
                            &drag_state_for_handle,
                        );
                        if should_start_scrollbar_poll {
                            Self::start_vertical_scrollbar_lazy_fetch_poll(
                                table_for_handle.clone(),
                                lazy_fetch_session_for_handle.clone(),
                                lazy_fetch_callback_for_handle.clone(),
                                lazy_fetch_more_in_flight_for_handle.clone(),
                                drag_state_for_handle.clone(),
                            );
                        }
                        return false;
                    }
                    // Let FLTK handle clicks on embedded scrollbar widgets.
                    if Self::is_mouse_on_table_scrollbar(&table_for_handle, mouse_x, mouse_y) {
                        let mut state = drag_state_for_handle
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        state.vertical_scrollbar_sequence = false;
                        state.vertical_scrollbar_fetch_requested = false;
                        state.shift_click_base_selection = None;
                        state.pending_shift_click_selection = None;
                        return false;
                    }
                    let button = app::event_button();
                    if button == app::MouseButton::Right as i32 {
                        {
                            let mut state = drag_state_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            state.shift_click_base_selection = None;
                            state.pending_shift_click_selection = None;
                        }
                        Self::show_context_menu(
                            &table_for_handle,
                            &headers_for_handle,
                            &full_data_for_handle,
                            &hidden_auto_rowid_col_for_handle,
                            &source_sql_for_handle,
                            &execute_sql_callback_for_handle,
                            &edit_session_for_handle,
                            &pending_save_request_for_handle,
                            &active_inline_edit_for_handle,
                            &lazy_fetch_session_for_handle,
                            &lazy_fetch_callback_for_handle,
                            &pending_lazy_actions_for_handle,
                            &context_action_callback_for_handle,
                        );
                        return true;
                    }
                    // Left click - start drag selection
                    if button == app::MouseButton::Left as i32 {
                        let _ = table_for_handle.take_focus();
                        let shift = app::event_state().contains(Shortcut::Shift);
                        if let Some(col) = Self::get_col_header_at_mouse(&table_for_handle) {
                            if col >= 0 {
                                let mut state = drag_state_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                state.header_sort_candidate_col = Some(col);
                                state.header_sort_requires_double_click = app::event_clicks();
                                state.header_sort_start_x = app::event_x();
                                state.header_sort_start_y = app::event_y();
                                state.is_dragging = false;
                                state.consume_background_pointer_sequence = false;
                                state.vertical_scrollbar_sequence = false;
                                state.vertical_scrollbar_fetch_requested = false;
                                state.shift_click_base_selection = None;
                                state.pending_shift_click_selection = None;
                                return true;
                            }
                        }
                        {
                            let mut state = drag_state_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            state.header_sort_candidate_col = None;
                            state.header_sort_requires_double_click = false;
                            state.consume_background_pointer_sequence = false;
                            state.vertical_scrollbar_sequence = false;
                            state.vertical_scrollbar_fetch_requested = false;
                            if !shift {
                                state.shift_click_base_selection = None;
                            }
                            state.pending_shift_click_selection = None;
                        }
                        let target_cell = if app::event_clicks() {
                            // On double-click, prefer the already-selected single cell.
                            // This avoids running a full mouse hit-test scan twice.
                            Self::resolve_double_click_target_cell(&table_for_handle)
                                .or_else(|| Self::get_cell_at_mouse(&table_for_handle))
                        } else if shift {
                            Self::get_cell_at_mouse(&table_for_handle)
                        } else {
                            Self::native_selected_cell_after_push(&table_for_handle)
                        };

                        if let Some((row, col)) = target_cell {
                            if app::event_clicks() {
                                // Clone the cell value before entering the modal dialog
                                // event loop so the full_data lock is released first.
                                // Use try_lock() so a streaming flush that is currently
                                // mutating the backing data never blocks the UI thread.
                                let current_font_profile = font_settings_for_handle.profile();
                                let current_font_size = font_settings_for_handle.size();
                                if Self::try_edit_cell_in_edit_mode(
                                    &table_for_handle,
                                    &headers_for_handle,
                                    &full_data_for_handle,
                                    &edit_session_for_handle,
                                    &pending_save_request_for_handle,
                                    &active_inline_edit_for_handle,
                                    row,
                                    col,
                                    current_font_profile,
                                    current_font_size,
                                ) {
                                    return true;
                                }

                                let cell_val_owned =
                                    Self::try_clone_cell_value(&full_data_for_handle, row, col);
                                if let Some(cell_val) = cell_val_owned {
                                    let current_font_profile = font_settings_for_handle.profile();
                                    let current_font_size = font_settings_for_handle.size();
                                    Self::show_cell_text_dialog(
                                        &cell_val,
                                        current_font_profile,
                                        current_font_size,
                                    );
                                    return true;
                                }
                            }
                            let max_rows = table_for_handle.rows().max(0) as usize;
                            let max_cols = table_for_handle.cols().max(0) as usize;
                            let base_selection = if shift {
                                let mut state = drag_state_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                let base_selection =
                                    state.shift_click_base_selection.or(state.last_selection);
                                if state.shift_click_base_selection.is_none() {
                                    state.shift_click_base_selection = state.last_selection;
                                }
                                base_selection
                            } else {
                                None
                            };
                            let next_selection = Self::shift_click_selection_with_cell(
                                base_selection,
                                row,
                                col,
                                max_rows,
                                max_cols,
                            );
                            let mut state = drag_state_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            state.is_dragging = true;
                            state.consume_background_pointer_sequence = false;
                            state.vertical_scrollbar_sequence = false;
                            state.vertical_scrollbar_fetch_requested = false;
                            state.pending_shift_click_selection =
                                if shift { next_selection } else { None };
                            drop(state);

                            if let Some((row_start, col_start, row_end, col_end)) = next_selection {
                                table_for_handle
                                    .set_selection(row_start, col_start, row_end, col_end);
                                Self::store_oriented_selection_snapshot(
                                    &table_for_handle,
                                    &drag_state_for_handle,
                                    (row_start, col_start, row_end, col_end),
                                );
                                return true;
                            }
                        }
                        let pointer_in_table_bounds = Self::is_mouse_within_bounds(
                            app::event_x(),
                            app::event_y(),
                            table_for_handle.x(),
                            table_for_handle.y(),
                            table_for_handle.w(),
                            table_for_handle.h(),
                        );
                        let row_header_hit = Self::get_row_header_at_mouse(&table_for_handle);
                        if Self::should_consume_background_pointer_sequence(
                            pointer_in_table_bounds,
                            false,
                            None,
                            row_header_hit,
                            None,
                        ) {
                            let mut state = drag_state_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            state.is_dragging = false;
                            state.consume_background_pointer_sequence = true;
                            state.vertical_scrollbar_sequence = false;
                            state.vertical_scrollbar_fetch_requested = false;
                            state.shift_click_base_selection = None;
                            state.pending_shift_click_selection = None;
                            drop(state);

                            if table_for_handle.try_get_selection().is_some() {
                                table_for_handle.unset_selection();
                                let mut state = drag_state_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                state.last_selection = None;
                                state.last_col_position = None;
                                table_for_handle.redraw();
                            }
                            return true;
                        }
                    }
                    false
                }
                Event::Drag => {
                    let (
                        is_dragging,
                        consume_background_pointer_sequence,
                        vertical_scrollbar_sequence,
                        header_sort_candidate,
                        header_sort_requires_double_click,
                        header_start_x,
                        header_start_y,
                    ) = {
                        let state = drag_state_for_handle
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        (
                            state.is_dragging,
                            state.consume_background_pointer_sequence,
                            state.vertical_scrollbar_sequence,
                            state.header_sort_candidate_col,
                            state.header_sort_requires_double_click,
                            state.header_sort_start_x,
                            state.header_sort_start_y,
                        )
                    };
                    if vertical_scrollbar_sequence {
                        Self::request_lazy_fetch_more_for_scrollbar_sequence(
                            &table_for_handle,
                            &lazy_fetch_session_for_handle,
                            &lazy_fetch_callback_for_handle,
                            &lazy_fetch_more_in_flight_for_handle,
                            &drag_state_for_handle,
                        );
                        return false;
                    }
                    if header_sort_candidate.is_some() {
                        if !header_sort_requires_double_click {
                            let mut state = drag_state_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            state.header_sort_candidate_col = None;
                            return true;
                        }
                        let moved_beyond = Self::pointer_moved_beyond_tolerance(
                            header_start_x,
                            header_start_y,
                            app::event_x(),
                            app::event_y(),
                            HEADER_SORT_CLICK_MOVE_TOLERANCE_PX,
                        );
                        if moved_beyond {
                            let mut state = drag_state_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            state.header_sort_candidate_col = None;
                            state.header_sort_requires_double_click = false;
                        } else {
                            return true;
                        }
                    }
                    if consume_background_pointer_sequence {
                        return true;
                    }
                    if is_dragging {
                        // Native Fl_Table::handle(Drag) already ran (super_handle_first=true)
                        // and extended the selection from Push origin to the current cell,
                        // including auto-scroll when the pointer leaves the visible area.
                        // No custom set_selection needed — just acknowledge the event.
                        return true;
                    }
                    false
                }
                Event::Released => {
                    let (
                        header_sort_candidate,
                        header_sort_requires_double_click,
                        was_dragging,
                        consumed_background_pointer_sequence,
                        vertical_scrollbar_sequence,
                        vertical_scrollbar_fetch_requested,
                        pending_shift_click_selection,
                    ) = {
                        let mut state = drag_state_for_handle
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let header_candidate = state.header_sort_candidate_col.take();
                        let header_is_double_click = state.header_sort_requires_double_click;
                        state.header_sort_requires_double_click = false;
                        let vertical_scrollbar_sequence = state.vertical_scrollbar_sequence;
                        let vertical_scrollbar_fetch_requested =
                            state.vertical_scrollbar_fetch_requested;
                        let pending_shift_click_selection =
                            state.pending_shift_click_selection.take();
                        state.vertical_scrollbar_sequence = false;
                        state.vertical_scrollbar_polling = false;
                        state.vertical_scrollbar_fetch_requested = false;
                        let consumed_background = state.consume_background_pointer_sequence;
                        state.consume_background_pointer_sequence = false;
                        let dragging = state.is_dragging;
                        if dragging {
                            state.is_dragging = false;
                        }
                        (
                            header_candidate,
                            header_is_double_click,
                            dragging,
                            consumed_background,
                            vertical_scrollbar_sequence,
                            vertical_scrollbar_fetch_requested,
                            pending_shift_click_selection,
                        )
                    };
                    if vertical_scrollbar_sequence {
                        if !vertical_scrollbar_fetch_requested {
                            Self::request_lazy_fetch_more_near_bottom(
                                &table_for_handle,
                                &lazy_fetch_session_for_handle,
                                &lazy_fetch_callback_for_handle,
                                &lazy_fetch_more_in_flight_for_handle,
                            );
                        }
                        return false;
                    }
                    if let Some(col) = header_sort_candidate {
                        if col >= 0
                            && header_sort_requires_double_click
                            && Self::get_col_header_at_mouse(&table_for_handle) == Some(col)
                            && Self::queue_lazy_action_if_active(
                                &lazy_fetch_session_for_handle,
                                &lazy_fetch_callback_for_handle,
                                &pending_lazy_actions_for_handle,
                                LazyFetchPendingAction::HeaderSort(col as usize),
                            )
                        {
                            return true;
                        }
                        if col >= 0
                            && header_sort_requires_double_click
                            && Self::get_col_header_at_mouse(&table_for_handle) == Some(col)
                            // Streaming append mutates full_data incrementally, so block
                            // column sort until the result set is finalized.
                            && !mutex_load_bool(&streaming_in_progress_for_handle)
                        {
                            let col_idx = col as usize;
                            let next_state = {
                                let current = *sort_state_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                Self::next_sort_state(current, col_idx)
                            };
                            if Self::apply_sort_to_table_data(
                                &full_data_for_handle,
                                &edit_session_for_handle,
                                col_idx,
                                next_state.direction,
                            ) {
                                *sort_state_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                    Some(next_state);
                                table_for_handle.redraw();
                            }
                        }
                        return true;
                    }
                    if was_dragging || consumed_background_pointer_sequence {
                        if was_dragging {
                            if let Some(selection) = pending_shift_click_selection {
                                table_for_handle.set_selection(
                                    selection.0,
                                    selection.1,
                                    selection.2,
                                    selection.3,
                                );
                                table_for_handle.redraw();
                                Self::store_oriented_selection_snapshot(
                                    &table_for_handle,
                                    &drag_state_for_handle,
                                    selection,
                                );
                            } else {
                                Self::sync_drag_selection_snapshot(
                                    &table_for_handle,
                                    &drag_state_for_handle,
                                );
                            }
                        }
                        return true;
                    }
                    false
                }
                Event::KeyDown => {
                    let key = app::event_key();
                    let original_key = app::event_original_key();
                    let state = app::event_state();
                    let ctrl_or_cmd =
                        state.contains(Shortcut::Ctrl) || state.contains(Shortcut::Command);
                    let shift = state.contains(Shortcut::Shift);

                    let home_end_nav_key =
                        Self::arrow_key_for_home_end(key, original_key, ctrl_or_cmd);
                    if ctrl_or_cmd || home_end_nav_key.is_some() {
                        let (nav_key, nav_original_key) = match home_end_nav_key {
                            Some(arrow) => (arrow, arrow),
                            None => (key, original_key),
                        };
                        let hidden_col = *hidden_auto_rowid_col_for_handle
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let (previous_selection, previous_col_position) = {
                            let state = drag_state_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            (state.last_selection, state.last_col_position)
                        };
                        let handled = if shift {
                            Self::handle_ctrl_shift_arrow_selection(
                                &mut table_for_handle,
                                nav_key,
                                nav_original_key,
                                hidden_col,
                                previous_selection,
                                previous_col_position,
                                &drag_state_for_handle,
                                &lazy_fetch_session_for_handle,
                                &lazy_fetch_callback_for_handle,
                                &pending_lazy_actions_for_handle,
                            )
                        } else {
                            Self::handle_ctrl_arrow_navigation(
                                &mut table_for_handle,
                                nav_key,
                                nav_original_key,
                                hidden_col,
                                previous_selection,
                                previous_col_position,
                                &lazy_fetch_session_for_handle,
                                &lazy_fetch_callback_for_handle,
                                &pending_lazy_actions_for_handle,
                            )
                        };
                        if handled {
                            if !shift {
                                Self::sync_drag_selection_snapshot(
                                    &table_for_handle,
                                    &drag_state_for_handle,
                                );
                            }
                            return true;
                        }
                    }

                    if Self::key_should_request_lazy_fetch_more(key) {
                        Self::request_lazy_fetch_more_near_bottom(
                            &table_for_handle,
                            &lazy_fetch_session_for_handle,
                            &lazy_fetch_callback_for_handle,
                            &lazy_fetch_more_in_flight_for_handle,
                        );
                    }

                    // PageUp/PageDown move the selection through the native Fl_Table
                    // handler (super_handle_first=true) without going through our
                    // navigation branches, so the snapshot is never refreshed. Sync it
                    // here so a following Ctrl+Left/Right (or Home/End) navigates from
                    // the current row, not the pre-paging one.
                    if matches!(key, Key::PageUp | Key::PageDown) {
                        Self::sync_drag_selection_snapshot(
                            &table_for_handle,
                            &drag_state_for_handle,
                        );
                        return false;
                    }

                    if matches!(key, Key::Left | Key::Right | Key::Up | Key::Down) {
                        let hidden_col = *hidden_auto_rowid_col_for_handle
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if shift && !ctrl_or_cmd {
                            let previous_selection = drag_state_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .last_selection;
                            let max_rows = table_for_handle.rows().max(0) as usize;
                            let max_cols = table_for_handle.cols().max(0) as usize;
                            if let Some(bounds) = Self::hidden_left_boundary_shift_restore_bounds(
                                previous_selection,
                                table_for_handle.get_selection(),
                                max_rows,
                                max_cols,
                                hidden_col,
                                key,
                                original_key,
                            ) {
                                Self::set_table_selection_bounds(
                                    &mut table_for_handle,
                                    bounds,
                                    ResultTableEdge::Left,
                                    None,
                                );
                                Self::sync_drag_selection_snapshot(
                                    &table_for_handle,
                                    &drag_state_for_handle,
                                );
                                return true;
                            }
                        }
                        if Self::clamp_selection_to_visible_columns(
                            &mut table_for_handle,
                            hidden_col,
                        ) {
                            Self::sync_drag_selection_snapshot(
                                &table_for_handle,
                                &drag_state_for_handle,
                            );
                            table_for_handle.redraw();
                            return true;
                        }
                        Self::sync_drag_selection_snapshot(
                            &table_for_handle,
                            &drag_state_for_handle,
                        );
                        return Self::should_consume_boundary_arrow(
                            &table_for_handle,
                            key,
                            hidden_col,
                        );
                    }

                    if ctrl_or_cmd {
                        if key == Key::Delete || original_key == Key::Delete {
                            match Self::set_selected_cells_to_null_in_edit_mode(
                                &table_for_handle,
                                &full_data_for_handle,
                                &edit_session_for_handle,
                                &pending_save_request_for_handle,
                                &active_inline_edit_for_handle,
                            ) {
                                Ok(_) => return true,
                                Err(err) => {
                                    if err.is_empty() {
                                        return false;
                                    }
                                    crate::ui::alert_on_main(&err);
                                    return true;
                                }
                            }
                        }
                        if shift && Self::matches_shortcut_key(key, original_key, 'c') {
                            Self::copy_selected_with_headers(
                                &table_for_handle,
                                &headers_for_handle,
                                &full_data_for_handle,
                                *hidden_auto_rowid_col_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
                            );
                            return true;
                        }
                        if Self::matches_shortcut_key(key, original_key, 'a') {
                            if Self::queue_lazy_action_if_active(
                                &lazy_fetch_session_for_handle,
                                &lazy_fetch_callback_for_handle,
                                &pending_lazy_actions_for_handle,
                                LazyFetchPendingAction::SelectAll,
                            ) {
                                return true;
                            }
                            let rows = table_for_handle.rows();
                            let cols = table_for_handle.cols();
                            if rows > 0 && cols > 0 {
                                table_for_handle.set_selection(0, 0, rows - 1, cols - 1);
                                Self::sync_drag_selection_snapshot(
                                    &table_for_handle,
                                    &drag_state_for_handle,
                                );
                                table_for_handle.redraw();
                            }
                            return true;
                        }
                        if Self::matches_shortcut_key(key, original_key, 'c') {
                            Self::copy_selected_to_clipboard(
                                &table_for_handle,
                                &full_data_for_handle,
                                *hidden_auto_rowid_col_for_handle
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
                            );
                            return true;
                        }
                        if Self::matches_shortcut_key(key, original_key, 'v') {
                            app::paste_text(&table_for_handle);
                            return true;
                        }
                    }

                    if matches!(key, Key::Enter | Key::KPEnter | Key::F2) {
                        let can_edit = Self::resolve_update_target_cell(
                            table_for_handle.get_selection(),
                            table_for_handle.rows().max(0) as usize,
                            table_for_handle.cols().max(0) as usize,
                            None,
                        )
                        .is_some()
                            && edit_session_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .is_some();

                        if can_edit {
                            let current_font_profile = font_settings_for_handle.profile();
                            let current_font_size = font_settings_for_handle.size();
                            if let Some((row, col)) = Self::resolve_update_target_cell(
                                table_for_handle.get_selection(),
                                table_for_handle.rows().max(0) as usize,
                                table_for_handle.cols().max(0) as usize,
                                None,
                            ) {
                                if Self::try_edit_cell_in_edit_mode(
                                    &table_for_handle,
                                    &headers_for_handle,
                                    &full_data_for_handle,
                                    &edit_session_for_handle,
                                    &pending_save_request_for_handle,
                                    &active_inline_edit_for_handle,
                                    row as i32,
                                    col as i32,
                                    current_font_profile,
                                    current_font_size,
                                ) {
                                    return true;
                                }
                            }
                            return false;
                        }
                    }

                    false
                }
                Event::Shortcut => {
                    let key = app::event_key();
                    let original_key = app::event_original_key();
                    let state = app::event_state();
                    let ctrl_or_cmd =
                        state.contains(Shortcut::Ctrl) || state.contains(Shortcut::Command);
                    let shift = state.contains(Shortcut::Shift);

                    let home_end_nav_key =
                        Self::arrow_key_for_home_end(key, original_key, ctrl_or_cmd);
                    if ctrl_or_cmd || home_end_nav_key.is_some() {
                        let (nav_key, nav_original_key) = match home_end_nav_key {
                            Some(arrow) => (arrow, arrow),
                            None => (key, original_key),
                        };
                        let hidden_col = *hidden_auto_rowid_col_for_handle
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let (previous_selection, previous_col_position) = {
                            let state = drag_state_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            (state.last_selection, state.last_col_position)
                        };
                        let handled = if shift {
                            Self::handle_ctrl_shift_arrow_selection(
                                &mut table_for_handle,
                                nav_key,
                                nav_original_key,
                                hidden_col,
                                previous_selection,
                                previous_col_position,
                                &drag_state_for_handle,
                                &lazy_fetch_session_for_handle,
                                &lazy_fetch_callback_for_handle,
                                &pending_lazy_actions_for_handle,
                            )
                        } else {
                            Self::handle_ctrl_arrow_navigation(
                                &mut table_for_handle,
                                nav_key,
                                nav_original_key,
                                hidden_col,
                                previous_selection,
                                previous_col_position,
                                &lazy_fetch_session_for_handle,
                                &lazy_fetch_callback_for_handle,
                                &pending_lazy_actions_for_handle,
                            )
                        };
                        if handled {
                            if !shift {
                                Self::sync_drag_selection_snapshot(
                                    &table_for_handle,
                                    &drag_state_for_handle,
                                );
                            }
                            return true;
                        }
                    }

                    if ctrl_or_cmd && shift && Self::matches_shortcut_key(key, original_key, 'c') {
                        Self::copy_selected_with_headers(
                            &table_for_handle,
                            &headers_for_handle,
                            &full_data_for_handle,
                            *hidden_auto_rowid_col_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()),
                        );
                        return true;
                    }
                    if ctrl_or_cmd && (key == Key::Delete || original_key == Key::Delete) {
                        match Self::set_selected_cells_to_null_in_edit_mode(
                            &table_for_handle,
                            &full_data_for_handle,
                            &edit_session_for_handle,
                            &pending_save_request_for_handle,
                            &active_inline_edit_for_handle,
                        ) {
                            Ok(_) => return true,
                            Err(err) => {
                                if err.is_empty() {
                                    return false;
                                }
                                crate::ui::alert_on_main(&err);
                                return true;
                            }
                        }
                    }
                    if ctrl_or_cmd && Self::matches_shortcut_key(key, original_key, 'c') {
                        Self::copy_selected_to_clipboard(
                            &table_for_handle,
                            &full_data_for_handle,
                            *hidden_auto_rowid_col_for_handle
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()),
                        );
                        return true;
                    }
                    if ctrl_or_cmd && Self::matches_shortcut_key(key, original_key, 'a') {
                        if Self::queue_lazy_action_if_active(
                            &lazy_fetch_session_for_handle,
                            &lazy_fetch_callback_for_handle,
                            &pending_lazy_actions_for_handle,
                            LazyFetchPendingAction::SelectAll,
                        ) {
                            return true;
                        }
                        let rows = table_for_handle.rows();
                        let cols = table_for_handle.cols();
                        if rows > 0 && cols > 0 {
                            table_for_handle.set_selection(0, 0, rows - 1, cols - 1);
                            Self::sync_drag_selection_snapshot(
                                &table_for_handle,
                                &drag_state_for_handle,
                            );
                            table_for_handle.redraw();
                        }
                        return true;
                    }
                    if ctrl_or_cmd && Self::matches_shortcut_key(key, original_key, 'v') {
                        app::paste_text(&table_for_handle);
                        return true;
                    }
                    false
                }
                Event::MouseWheel => {
                    Self::request_lazy_fetch_more_near_bottom(
                        &table_for_handle,
                        &lazy_fetch_session_for_handle,
                        &lazy_fetch_callback_for_handle,
                        &lazy_fetch_more_in_flight_for_handle,
                    );
                    false
                }
                Event::Paste => {
                    let pasted_text = app::event_text();
                    match Self::paste_clipboard_text_into_edit_mode(
                        &table_for_handle,
                        &full_data_for_handle,
                        &edit_session_for_handle,
                        &pending_save_request_for_handle,
                        &active_inline_edit_for_handle,
                        &pasted_text,
                    ) {
                        Ok(_) => true,
                        Err(err) => {
                            // Clipboard text can be delivered to this widget via non-edit paths
                            // (e.g. drag/drop). Only surface actionable errors to users.
                            if !err.is_empty() {
                                crate::ui::alert_on_main(&err);
                            }
                            true
                        }
                    }
                }
                Event::Resize | Event::Move => {
                    // Main window/layout resizes can shift the table's viewport without
                    // reliably invoking the widget resize callback in time for the inline
                    // editor overlay. Re-anchor the active editor here as well so it stays
                    // aligned to the edited cell.
                    Self::reposition_active_inline_editor(
                        &table_for_handle,
                        &active_inline_edit_for_handle,
                    );
                    false
                }
                _ => false,
            }
        });

        table.resize_callback(move |table_widget, _, _, _, _| {
            Self::reposition_active_inline_editor(table_widget, &active_inline_edit_for_resize);
        });

        Self {
            table,
            headers,
            pending_rows: Arc::new(Mutex::new(Vec::new())),
            pending_widths: Arc::new(Mutex::new(Vec::new())),
            last_flush_epoch_ms: Arc::new(Mutex::new(Self::current_epoch_millis())),
            full_data,
            max_cell_display_chars,
            max_cell_display_chars_draw,
            width_sampled_rows: Arc::new(Mutex::new(0_usize)),
            font_settings,
            null_text,
            source_sql,
            execute_sql_callback,
            edit_session,
            query_edit_backup,
            pending_save_request,
            pending_save_sql_signature,
            pending_save_request_tag,
            pending_save_statement_signatures,
            next_save_request_id,
            hidden_auto_rowid_col,
            active_inline_edit,
            streaming_in_progress,
            lazy_fetch_session,
            lazy_fetch_callback,
            lazy_fetch_more_in_flight,
            pending_lazy_actions,
            context_action_callback,
            sort_state,
            drag_state,
        }
    }

    fn is_streaming_in_progress(&self) -> bool {
        mutex_load_bool(&self.streaming_in_progress)
    }

    fn invoke_lazy_fetch_callback(
        callback: &LazyFetchCallback,
        session_id: u64,
        request: LazyFetchRequest,
    ) -> bool {
        let mut callback_fn = callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let accepted = callback_fn
            .as_mut()
            .is_some_and(|callback_fn| callback_fn(session_id, request));
        let mut callback_guard = callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if callback_guard.is_none() {
            *callback_guard = callback_fn;
        }
        accepted
    }

    fn invoke_context_action_callback(
        callback: &ResultTableContextActionCallback,
        action: ResultTableContextAction,
    ) {
        let mut callback_fn = callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(callback_fn) = callback_fn.as_mut() {
            callback_fn(action);
        }
        let mut callback_guard = callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if callback_guard.is_none() {
            *callback_guard = callback_fn;
        }
    }

    fn schedule_context_action_callback(
        callback: &ResultTableContextActionCallback,
        action: ResultTableContextAction,
    ) {
        let callback = callback.clone();
        crate::ui::ui_timeout::schedule(0.0, move || {
            Self::invoke_context_action_callback(&callback, action);
        });
    }

    fn queue_lazy_action_if_active(
        session: &Arc<Mutex<Option<u64>>>,
        callback: &LazyFetchCallback,
        pending_actions: &Arc<Mutex<VecDeque<(u64, LazyFetchPendingAction)>>>,
        action: LazyFetchPendingAction,
    ) -> bool {
        let session_id = *session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(session_id) = session_id {
            if !Self::invoke_lazy_fetch_callback(callback, session_id, LazyFetchRequest::All) {
                return false;
            }
            pending_actions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push_back((session_id, action));
            true
        } else {
            false
        }
    }

    fn current_csv_snapshot(&self) -> (String, usize) {
        (self.build_csv_snapshot(), self.row_count())
    }

    fn is_table_near_bottom(table: &Table) -> bool {
        let rows = table.rows();
        if rows <= 0 {
            return false;
        }
        let first_visible = table.row_position().max(0);
        let row_h = Self::uniform_row_height_near_viewport(table, first_visible, rows)
            .unwrap_or_else(|| table.row_height(first_visible));
        if row_h <= 0 {
            return false;
        }
        let data_top = table.y().saturating_add(table.col_header_height());
        let data_bottom = table.y().saturating_add(table.h());
        let visible_rows = Self::visible_row_metrics(data_top, data_bottom, row_h)
            .map(|(rows, _)| rows)
            .unwrap_or(1);
        Self::is_lazy_fetch_scroll_threshold_reached(first_visible, visible_rows, rows)
    }

    fn is_lazy_fetch_scroll_threshold_reached(
        first_visible: i32,
        visible_rows: i32,
        rows: i32,
    ) -> bool {
        if rows <= 0 {
            return false;
        }
        let visible_rows = visible_rows.max(1);
        let max_scroll_start = rows.saturating_sub(visible_rows);
        if max_scroll_start <= 0 {
            return true;
        }

        let first_visible = first_visible.clamp(0, max_scroll_start);
        i64::from(first_visible) * LAZY_FETCH_SCROLL_THRESHOLD_DENOMINATOR
            >= i64::from(max_scroll_start) * LAZY_FETCH_SCROLL_THRESHOLD_NUMERATOR
    }

    fn request_lazy_fetch_more_for_session(
        session_id: u64,
        session: &Arc<Mutex<Option<u64>>>,
        callback: &LazyFetchCallback,
        in_flight: &Arc<Mutex<HashSet<u64>>>,
    ) -> bool {
        let active_session_id = *session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active_session_id != Some(session_id) {
            return false;
        }

        let mut guard = in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !guard.insert(session_id) {
            return false;
        }
        drop(guard);
        if Self::invoke_lazy_fetch_callback(callback, session_id, LazyFetchRequest::More) {
            true
        } else {
            in_flight
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&session_id);
            false
        }
    }

    fn request_lazy_fetch_more_near_bottom(
        table: &Table,
        session: &Arc<Mutex<Option<u64>>>,
        callback: &LazyFetchCallback,
        in_flight: &Arc<Mutex<HashSet<u64>>>,
    ) -> bool {
        let session_id = *session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(session_id) = session_id else {
            return false;
        };
        if Self::is_table_near_bottom(table) {
            return Self::request_lazy_fetch_more_for_session(
                session_id, session, callback, in_flight,
            );
        }
        false
    }

    fn request_lazy_fetch_more_for_scrollbar_sequence(
        table: &Table,
        session: &Arc<Mutex<Option<u64>>>,
        callback: &LazyFetchCallback,
        in_flight: &Arc<Mutex<HashSet<u64>>>,
        drag_state: &Arc<Mutex<DragState>>,
    ) {
        if !Self::is_table_near_bottom(table) {
            drag_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .vertical_scrollbar_fetch_requested = false;
            return;
        }
        {
            let mut state = drag_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.vertical_scrollbar_fetch_requested {
                return;
            }
            state.vertical_scrollbar_fetch_requested = true;
        }
        if !Self::request_lazy_fetch_more_near_bottom(table, session, callback, in_flight) {
            drag_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .vertical_scrollbar_fetch_requested = false;
        }
    }

    fn start_vertical_scrollbar_lazy_fetch_poll(
        table: Table,
        session: Arc<Mutex<Option<u64>>>,
        callback: LazyFetchCallback,
        in_flight: Arc<Mutex<HashSet<u64>>>,
        drag_state: Arc<Mutex<DragState>>,
    ) {
        crate::ui::ui_timeout::schedule(LAZY_FETCH_SCROLLBAR_POLL_INTERVAL_SECONDS, move || {
            let should_continue = {
                let mut state = drag_state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if table.was_deleted()
                    || !state.vertical_scrollbar_sequence
                    || app::pushed().is_none()
                {
                    state.vertical_scrollbar_sequence = false;
                    state.vertical_scrollbar_polling = false;
                    state.vertical_scrollbar_fetch_requested = false;
                    false
                } else {
                    true
                }
            };
            if !should_continue {
                return;
            }

            Self::request_lazy_fetch_more_for_scrollbar_sequence(
                &table,
                &session,
                &callback,
                &in_flight,
                &drag_state,
            );
            Self::start_vertical_scrollbar_lazy_fetch_poll(
                table.clone(),
                session.clone(),
                callback.clone(),
                in_flight.clone(),
                drag_state.clone(),
            );
        });
    }

    fn key_should_request_lazy_fetch_more(key: Key) -> bool {
        matches!(key, Key::PageDown | Key::End | Key::Down)
    }

    fn show_inline_cell_editor(
        table: &Table,
        row: i32,
        col: i32,
        current_value: &str,
        font_profile: FontProfile,
        font_size: u32,
        active_inline_edit: &Arc<Mutex<Option<ActiveInlineEdit>>>,
    ) -> Option<String> {
        let Some((x, y, w, h)) = table.find_cell(TableContext::Cell, row, col) else {
            return crate::ui::input_on_main(
                "Value (blank/NULL -> NULL, '=expr' -> SQL)",
                current_value,
            );
        };

        let current_group = Group::try_current();
        let parent_group = table.parent();
        if let Some(ref parent) = parent_group {
            Group::set_current(Some(parent));
        } else {
            Group::set_current(None::<&Group>);
        }

        let input_x = x + 1;
        let input_y = y + 1;
        let input_w = (w - 2).max(24);
        let input_h = (h - 2).max(24);
        let mut input = Input::new(input_x, input_y, input_w, input_h, None);
        Group::set_current(current_group.as_ref());

        input.set_color(theme::input_bg());
        input.set_text_color(theme::text_primary());
        input.set_text_font(font_profile.normal);
        input.set_text_size(font_size as i32);
        input.set_value(current_value);
        input.set_trigger(CallbackTrigger::EnterKeyAlways);

        if let (Ok(row_idx), Ok(col_idx)) = (usize::try_from(row), usize::try_from(col)) {
            *active_inline_edit
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ActiveInlineEdit {
                row: row_idx,
                col: col_idx,
                input: input.clone(),
            });
        }

        let result: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let finished = Arc::new(Mutex::new(false));
        let cancelled = Arc::new(Mutex::new(false));

        {
            let result = result.clone();
            let finished = finished.clone();
            let input_value = input.clone();
            input.set_callback(move |_| {
                *result
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(input_value.value());
                *finished
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
            });
        }

        {
            let result = result.clone();
            let finished = finished.clone();
            let cancelled = cancelled.clone();
            let mut input_handle = input.clone();
            let table_for_mouse_bounds = table.clone();
            input_handle.handle(move |widget, ev| match ev {
                Event::KeyDown if app::event_key() == Key::Escape => {
                    *cancelled
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
                    *finished
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
                    widget.hide();
                    true
                }
                Event::Move | Event::Drag | Event::Leave => {
                    if !Self::is_mouse_within_bounds(
                        app::event_x(),
                        app::event_y(),
                        table_for_mouse_bounds.x(),
                        table_for_mouse_bounds.y(),
                        table_for_mouse_bounds.w(),
                        table_for_mouse_bounds.h(),
                    ) {
                        let is_finished = *finished
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if !is_finished {
                            *result
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                Some(widget.value());
                            *finished
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
                        }
                    }
                    false
                }
                Event::Unfocus => {
                    let is_finished = *finished
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if !is_finished {
                        *result
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                            Some(widget.value());
                        *finished
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
                    }
                    false
                }
                _ => false,
            });
        }

        let _ = input.take_focus();
        while !*finished
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            if input.was_deleted() {
                *cancelled
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
                *finished
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
                break;
            }
            app::wait();
        }

        let was_cancelled = *cancelled
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let value = result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        *active_inline_edit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        if !input.was_deleted() && app::is_ui_thread() {
            Input::delete(input);
        }
        let mut table = table.clone();
        if !table.was_deleted() {
            let _ = table.take_focus();
            table.redraw();
        }
        if was_cancelled {
            None
        } else {
            Some(value.unwrap_or_default())
        }
    }

    fn try_edit_cell_in_edit_mode(
        table: &Table,
        headers: &Arc<Mutex<Vec<String>>>,
        full_data: &Arc<Mutex<Vec<Vec<String>>>>,
        edit_session: &Arc<Mutex<Option<TableEditSession>>>,
        pending_save_request: &Arc<Mutex<bool>>,
        active_inline_edit: &Arc<Mutex<Option<ActiveInlineEdit>>>,
        row: i32,
        col: i32,
        font_profile: FontProfile,
        font_size: u32,
    ) -> bool {
        let (row_idx, col_idx) = match (usize::try_from(row), usize::try_from(col)) {
            (Ok(r), Ok(c)) => (r, c),
            _ => return true,
        };

        let save_pending = *pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if save_pending {
            crate::ui::alert_on_main("Save is in progress. Wait for completion before editing.");
            return true;
        }

        let (rowid_col, is_editable_column) = {
            let guard = edit_session
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(session) = guard.as_ref() else {
                return false;
            };
            (
                session.rowid_col,
                Self::is_editable_column(session, col_idx),
            )
        };

        if col_idx == rowid_col {
            crate::ui::alert_on_main("ROWID column cannot be edited.");
            return true;
        }
        if !is_editable_column {
            // 편집 불가 컬럼은 alert 없이 false를 반환해 일반 셀 내용 팝업으로 fallthrough
            return false;
        }

        let column_exists = headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(col_idx)
            .is_some();
        if !column_exists {
            crate::ui::alert_on_main("Selected column is out of range.");
            return true;
        }

        let current_value = {
            let data = full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            data.get(row_idx)
                .and_then(|row_data| row_data.get(col_idx))
                .cloned()
                .unwrap_or_default()
        };

        let Some(new_value) = Self::show_inline_cell_editor(
            table,
            row,
            col,
            &current_value,
            font_profile,
            font_size,
            active_inline_edit,
        ) else {
            return true;
        };
        if new_value == current_value {
            return true;
        }

        {
            let mut data = full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(row_data) = data.get_mut(row_idx) else {
                drop(data);
                crate::ui::alert_on_main("Selected row is out of range.");
                return true;
            };
            if col_idx >= row_data.len() {
                row_data.resize(col_idx + 1, String::new());
            }
            row_data[col_idx] = new_value.clone();
        }
        {
            let mut guard = edit_session
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(session) = guard.as_mut() {
                let is_explicit_null = session
                    .row_states
                    .get(row_idx)
                    .map(|row_state| {
                        Self::input_maps_to_explicit_null(row_state, &new_value, &session.null_text)
                    })
                    .unwrap_or(false);
                let _ =
                    Self::set_row_cell_explicit_null(session, row_idx, col_idx, is_explicit_null);
                Self::sync_existing_row_dirty_cell(session, row_idx, col_idx, &new_value);
            }
        }
        let mut table = table.clone();
        table.redraw();
        true
    }

    fn commit_active_inline_edit(&mut self) {
        Self::commit_active_inline_edit_from_refs(
            &self.table,
            &self.full_data,
            &self.edit_session,
            &self.active_inline_edit,
        );
    }

    fn commit_active_inline_edit_from_refs(
        table: &Table,
        full_data: &Arc<Mutex<Vec<Vec<String>>>>,
        edit_session: &Arc<Mutex<Option<TableEditSession>>>,
        active_inline_edit: &Arc<Mutex<Option<ActiveInlineEdit>>>,
    ) {
        let active_editor = active_inline_edit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(active_editor) = active_editor else {
            return;
        };

        if !active_editor.input.was_deleted() {
            let new_value = active_editor.input.value();
            // Inline-edit commits are valid only while edit mode is active and
            // the editor still targets an editable column. Otherwise, discard
            // the transient input widget without mutating table data.
            let mut should_apply = false;
            {
                let session_guard = edit_session
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(session) = session_guard.as_ref() {
                    let is_editable_col = Self::is_editable_column(session, active_editor.col);
                    should_apply = active_editor.row < session.row_states.len()
                        && active_editor.col != session.rowid_col
                        && is_editable_col;
                }
            }
            if should_apply {
                // Update full_data in its own scope so the lock is released
                // before acquiring edit_session, keeping a consistent lock
                // order (edit_session → full_data) with the rest of the code.
                {
                    let mut full_data = full_data
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if let Some(row_data) = full_data.get_mut(active_editor.row) {
                        if active_editor.col >= row_data.len() {
                            row_data.resize(active_editor.col + 1, String::new());
                        }
                        row_data[active_editor.col] = new_value.clone();
                    }
                }
                {
                    let mut session_guard = edit_session
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if let Some(session) = session_guard.as_mut() {
                        let is_explicit_null = session
                            .row_states
                            .get(active_editor.row)
                            .map(|row_state| {
                                Self::input_maps_to_explicit_null(
                                    row_state,
                                    &new_value,
                                    &session.null_text,
                                )
                            })
                            .unwrap_or(false);
                        let _ = Self::set_row_cell_explicit_null(
                            session,
                            active_editor.row,
                            active_editor.col,
                            is_explicit_null,
                        );
                        Self::sync_existing_row_dirty_cell(
                            session,
                            active_editor.row,
                            active_editor.col,
                            &new_value,
                        );
                    }
                }
            }

            let mut input = active_editor.input;
            input.hide();
            if app::is_ui_thread() {
                Input::delete(input);
            }
        }

        *active_inline_edit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let mut table = table.clone();
        table.redraw();
    }

    fn matches_shortcut_key(key: Key, original_key: Key, ascii: char) -> bool {
        let lower = Key::from_char(ascii.to_ascii_lowercase());
        let upper = Key::from_char(ascii.to_ascii_uppercase());
        key == lower || key == upper || original_key == lower || original_key == upper
    }

    fn is_mouse_within_bounds(
        mouse_x: i32,
        mouse_y: i32,
        rect_x: i32,
        rect_y: i32,
        rect_w: i32,
        rect_h: i32,
    ) -> bool {
        if rect_w <= 0 || rect_h <= 0 {
            return false;
        }

        let right = rect_x.saturating_add(rect_w);
        let bottom = rect_y.saturating_add(rect_h);
        mouse_x >= rect_x && mouse_x < right && mouse_y >= rect_y && mouse_y < bottom
    }

    fn should_consume_background_pointer_sequence(
        pointer_in_table_bounds: bool,
        on_scrollbar: bool,
        cell_hit: Option<(i32, i32)>,
        row_header_hit: Option<i32>,
        col_header_hit: Option<i32>,
    ) -> bool {
        pointer_in_table_bounds
            && !on_scrollbar
            && cell_hit.is_none()
            && row_header_hit.is_none()
            && col_header_hit.is_none()
    }

    fn parse_clipboard_rows(clipboard_text: &str) -> Vec<Vec<String>> {
        if clipboard_text.is_empty() {
            return Vec::new();
        }
        // Tab-separated parse that honors double-quoted fields, so values
        // containing tabs/newlines (escaped by `escape_cell_for_clipboard` or by
        // spreadsheet apps) round-trip back into a single cell.
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut current_row: Vec<String> = Vec::new();
        let mut field = String::new();
        let mut in_quotes = false;
        let mut chars = clipboard_text.chars().peekable();
        while let Some(c) = chars.next() {
            if in_quotes {
                match c {
                    '"' => {
                        if chars.peek() == Some(&'"') {
                            field.push('"');
                            chars.next();
                        } else {
                            in_quotes = false;
                        }
                    }
                    _ => field.push(c),
                }
            } else {
                match c {
                    '"' if field.is_empty() => in_quotes = true,
                    '\t' => current_row.push(std::mem::take(&mut field)),
                    '\r' => {
                        if chars.peek() == Some(&'\n') {
                            chars.next();
                        }
                        current_row.push(std::mem::take(&mut field));
                        rows.push(std::mem::take(&mut current_row));
                    }
                    '\n' => {
                        current_row.push(std::mem::take(&mut field));
                        rows.push(std::mem::take(&mut current_row));
                    }
                    _ => field.push(c),
                }
            }
        }
        current_row.push(field);
        rows.push(current_row);
        while rows.len() > 1
            && rows
                .last()
                .and_then(|row| row.first().map(|first| row.len() == 1 && first.is_empty()))
                .unwrap_or(false)
        {
            rows.pop();
        }
        rows
    }

    fn resolve_paste_anchor_column(
        anchor_col: usize,
        selection: Option<(usize, usize, usize, usize)>,
        rowid_col: usize,
        editable_cols: &HashSet<usize>,
        max_cols: usize,
    ) -> Option<usize> {
        if max_cols == 0 {
            return None;
        }

        let is_editable_target = |col: usize| col != rowid_col && editable_cols.contains(&col);
        if anchor_col < max_cols && is_editable_target(anchor_col) {
            return Some(anchor_col);
        }

        if let Some((_, col_start, _, col_end)) = selection {
            let start = col_start.min(max_cols.saturating_sub(1));
            let end = col_end.min(max_cols.saturating_sub(1));
            for col in start..=end {
                if is_editable_target(col) {
                    return Some(col);
                }
            }
        }

        let start_right = anchor_col.saturating_add(1);
        if start_right < max_cols {
            for col in start_right..max_cols {
                if is_editable_target(col) {
                    return Some(col);
                }
            }
        }

        let left_end = anchor_col.min(max_cols.saturating_sub(1));
        (0..=left_end).find(|&col| is_editable_target(col))
    }

    /// Apply pasted values to the data grid.
    /// Returns `(changed_cells, skipped_cells, updated_cells)` where `skipped_cells` counts
    /// editable target cells that fell outside the current table bounds.
    fn apply_paste_values_to_data(
        full_data: &mut [Vec<String>],
        rowid_col: usize,
        editable_cols: &HashSet<usize>,
        max_cols: usize,
        anchor: (usize, usize),
        selection: Option<(usize, usize, usize, usize)>,
        pasted_rows: &[Vec<String>],
    ) -> (usize, usize, Vec<(usize, usize)>) {
        if pasted_rows.is_empty() {
            return (0, 0, Vec::new());
        }

        let mut changed_cells = 0usize;
        let mut skipped_cells = 0usize;
        let mut updated_cells = Vec::new();
        let row_count = full_data.len();
        let mut apply_value = |target_row: usize, target_col: usize, value: &str| {
            if target_col >= max_cols {
                skipped_cells = skipped_cells.saturating_add(1);
                return;
            }
            if target_col == rowid_col || !editable_cols.contains(&target_col) {
                return;
            }
            let Some(row) = full_data.get_mut(target_row) else {
                skipped_cells = skipped_cells.saturating_add(1);
                return;
            };
            if target_col >= row.len() {
                row.resize(target_col + 1, String::new());
            }
            if row
                .get(target_col)
                .map(|existing| existing != value)
                .unwrap_or(true)
            {
                row[target_col] = value.to_string();
                changed_cells = changed_cells.saturating_add(1);
                updated_cells.push((target_row, target_col));
            }
        };

        if pasted_rows.len() == 1 && pasted_rows.first().map(|r| r.len()).unwrap_or(0) == 1 {
            let fill_value = pasted_rows
                .first()
                .and_then(|row| row.first())
                .map(|s| s.as_str())
                .unwrap_or_default();
            if let Some((row_start, col_start, row_end, col_end)) = selection {
                for row_idx in row_start..=row_end {
                    for col_idx in col_start..=col_end {
                        apply_value(row_idx, col_idx, fill_value);
                    }
                }
                return (changed_cells, skipped_cells, updated_cells);
            }
            apply_value(anchor.0, anchor.1, fill_value);
            return (changed_cells, skipped_cells, updated_cells);
        }

        for (row_offset, source_row) in pasted_rows.iter().enumerate() {
            let Some(target_row) = anchor.0.checked_add(row_offset) else {
                continue;
            };
            if target_row >= row_count {
                // All remaining source rows are out of bounds; count their
                // editable cells as skipped and stop early.
                for remaining in pasted_rows.iter().skip(row_offset) {
                    for (col_offset, _) in remaining.iter().enumerate() {
                        if let Some(tc) = anchor.1.checked_add(col_offset) {
                            if tc != rowid_col && editable_cols.contains(&tc) {
                                skipped_cells = skipped_cells.saturating_add(1);
                            }
                        }
                    }
                }
                break;
            }
            for (col_offset, source_cell) in source_row.iter().enumerate() {
                let Some(target_col) = anchor.1.checked_add(col_offset) else {
                    continue;
                };
                apply_value(target_row, target_col, source_cell);
            }
        }

        (changed_cells, skipped_cells, updated_cells)
    }

    fn paste_clipboard_text_into_edit_mode(
        table: &Table,
        full_data: &Arc<Mutex<Vec<Vec<String>>>>,
        edit_session: &Arc<Mutex<Option<TableEditSession>>>,
        pending_save_request: &Arc<Mutex<bool>>,
        active_inline_edit: &Arc<Mutex<Option<ActiveInlineEdit>>>,
        clipboard_text: &str,
    ) -> Result<usize, String> {
        if *pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            return Err("Cannot paste while save is in progress.".to_string());
        }

        Self::commit_active_inline_edit_from_refs(
            table,
            full_data,
            edit_session,
            active_inline_edit,
        );

        let session = edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned()
            .ok_or_else(String::new)?;
        let anchor = Self::selected_anchor_cell(table)
            .ok_or_else(|| "Select a target cell in the result table first.".to_string())?;
        let pasted_rows = Self::parse_clipboard_rows(clipboard_text);
        if pasted_rows.is_empty() {
            return Err("Clipboard text is empty.".to_string());
        }

        let selection = Self::normalized_selection_bounds_with_limits(
            table.get_selection(),
            table.rows().max(0) as usize,
            table.cols().max(0) as usize,
        );
        let editable_cols: HashSet<usize> = session
            .editable_columns
            .iter()
            .map(|(col_idx, _)| *col_idx)
            .collect();
        let anchor_col = Self::resolve_paste_anchor_column(
            anchor.1,
            selection,
            session.rowid_col,
            &editable_cols,
            table.cols().max(0) as usize,
        )
        .ok_or_else(|| "No editable target column is selected for paste.".to_string())?;
        let anchor = (anchor.0, anchor_col);
        let (changed_cells, skipped_cells) = {
            let mut rows = full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut session_guard = edit_session
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(session_mut) = session_guard.as_mut() else {
                return Err("Enable edit mode first.".to_string());
            };
            if rows.len() != session_mut.row_states.len() {
                return Err("Edit session and table rows are out of sync.".to_string());
            }
            if rows.is_empty() {
                return Err("No staged rows are available for paste.".to_string());
            }
            let (changed_cells, skipped_cells, updated_cells) = Self::apply_paste_values_to_data(
                &mut rows,
                session_mut.rowid_col,
                &editable_cols,
                table.cols().max(0) as usize,
                anchor,
                selection,
                &pasted_rows,
            );

            for (row_idx, col_idx) in &updated_cells {
                let input_value = rows
                    .get(*row_idx)
                    .and_then(|row| row.get(*col_idx))
                    .cloned()
                    .unwrap_or_default();
                let is_explicit_null = session_mut
                    .row_states
                    .get(*row_idx)
                    .map(|row_state| {
                        Self::input_maps_to_explicit_null(
                            row_state,
                            &input_value,
                            &session_mut.null_text,
                        )
                    })
                    .unwrap_or(false);
                let _ = Self::set_row_cell_explicit_null(
                    session_mut,
                    *row_idx,
                    *col_idx,
                    is_explicit_null,
                );
                Self::sync_existing_row_dirty_cell(session_mut, *row_idx, *col_idx, &input_value);
            }
            (changed_cells, skipped_cells)
        };

        if changed_cells == 0 && skipped_cells == 0 {
            return Err("No editable cells were updated from pasted values.".to_string());
        }
        if changed_cells == 0 && skipped_cells > 0 {
            return Err(format!(
                "All {} pasted cell(s) fell outside the table bounds.",
                skipped_cells
            ));
        }

        let mut table = table.clone();
        table.redraw();
        if skipped_cells > 0 {
            crate::ui::alert_on_main(&format!(
                "Pasted {} cell(s), but {} cell(s) were skipped (outside table bounds).",
                changed_cells, skipped_cells
            ));
        }
        Ok(changed_cells)
    }

    fn set_selected_cells_to_null_in_edit_mode(
        table: &Table,
        full_data: &Arc<Mutex<Vec<Vec<String>>>>,
        edit_session: &Arc<Mutex<Option<TableEditSession>>>,
        pending_save_request: &Arc<Mutex<bool>>,
        active_inline_edit: &Arc<Mutex<Option<ActiveInlineEdit>>>,
    ) -> Result<usize, String> {
        if *pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            return Err("Cannot set NULL while save is in progress.".to_string());
        }

        Self::commit_active_inline_edit_from_refs(
            table,
            full_data,
            edit_session,
            active_inline_edit,
        );

        let selection = Self::normalized_selection_bounds_with_limits(
            table.get_selection(),
            table.rows().max(0) as usize,
            table.cols().max(0) as usize,
        )
        .ok_or_else(|| "Select cell(s) to set NULL.".to_string())?;

        let (row_start, col_start, row_end, col_end) = selection;
        let mut changed = 0usize;
        let mut target_cells = 0usize;
        {
            let mut rows = full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut session_guard = edit_session
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(session) = session_guard.as_mut() else {
                return Err("Enable edit mode first.".to_string());
            };

            if rows.len() != session.row_states.len() {
                return Err("Edit session and table rows are out of sync.".to_string());
            }
            if rows.is_empty() {
                return Err("No staged rows are available for Set Null.".to_string());
            }

            let editable_cols: HashSet<usize> = session
                .editable_columns
                .iter()
                .map(|(col_idx, _)| *col_idx)
                .collect();
            let null_marker = session.null_text.clone();

            for row_idx in row_start..=row_end {
                if row_idx >= rows.len() {
                    continue;
                }
                for col_idx in col_start..=col_end {
                    if col_idx == session.rowid_col || !editable_cols.contains(&col_idx) {
                        continue;
                    }
                    target_cells = target_cells.saturating_add(1);
                    let Some(row) = rows.get_mut(row_idx) else {
                        continue;
                    };
                    if col_idx >= row.len() {
                        row.resize(col_idx + 1, String::new());
                    }
                    let value_changed = row
                        .get(col_idx)
                        .map(|existing| existing != &null_marker)
                        .unwrap_or(true);
                    row[col_idx] = null_marker.clone();
                    let flag_changed =
                        Self::set_row_cell_explicit_null(session, row_idx, col_idx, true);
                    Self::sync_existing_row_dirty_cell(session, row_idx, col_idx, &null_marker);
                    if value_changed || flag_changed {
                        changed = changed.saturating_add(1);
                    }
                }
            }
        }

        if target_cells == 0 {
            return Err("No editable cells were selected for Set Null.".to_string());
        }
        if changed > 0 {
            let mut table = table.clone();
            table.redraw();
        }
        Ok(changed)
    }

    fn fallback_scan_start(start: i32, max_backtrack: i32) -> i32 {
        if start <= 0 {
            return 0;
        }

        let limit = max_backtrack.max(0);
        if start <= limit {
            0
        } else {
            start.saturating_sub(limit)
        }
    }

    fn uniform_row_height_near_viewport(table: &Table, start_row: i32, rows: i32) -> Option<i32> {
        if start_row < 0 || rows <= 0 || start_row >= rows {
            return None;
        }

        let row_h = table.row_height(start_row);
        if row_h <= 0 {
            return None;
        }

        let next_row = start_row.saturating_add(1);
        if next_row < rows {
            let next_h = table.row_height(next_row);
            if next_h > 0 && next_h != row_h {
                return None;
            }
        }

        Some(row_h)
    }

    fn visible_row_metrics(data_top: i32, data_bottom: i32, row_h: i32) -> Option<(i32, i32)> {
        if row_h <= 0 || data_bottom <= data_top {
            return None;
        }

        let visible_height = data_bottom.saturating_sub(data_top);
        let visible_rows = safe_div(
            visible_height.saturating_add(row_h).saturating_sub(1),
            row_h,
        )
        .max(1);
        let visible_bottom = data_top
            .saturating_add(visible_rows.saturating_mul(row_h))
            .min(data_bottom);
        Some((visible_rows, visible_bottom))
    }

    fn offscreen_skip_count(
        item_origin: i32,
        item_extent: i32,
        viewport_start: i32,
    ) -> Option<i32> {
        if item_extent <= 0 {
            return None;
        }

        let item_end = item_origin.saturating_add(item_extent);
        if item_end > viewport_start {
            return None;
        }

        let hidden_px = viewport_start.saturating_sub(item_end);
        Some(safe_div(hidden_px, item_extent).max(1))
    }

    fn estimate_uniform_row_candidate(
        start_row: i32,
        start_y: i32,
        row_h: i32,
        mouse_y: i32,
        rows: i32,
    ) -> Option<i32> {
        if start_row < 0 || row_h <= 0 || rows <= 0 || start_row >= rows {
            return None;
        }

        let delta = mouse_y.saturating_sub(start_y);
        let last_row = rows.saturating_sub(1);
        Some(
            start_row
                .saturating_add(safe_div(delta, row_h))
                .max(0)
                .min(last_row),
        )
    }

    fn estimate_row_hit(
        table: &Table,
        context: TableContext,
        start_row: i32,
        anchor_col: i32,
        mouse_y: i32,
        rows: i32,
    ) -> Option<i32> {
        if start_row < 0 || anchor_col < 0 || rows <= 0 || start_row >= rows {
            return None;
        }

        if matches!(context, TableContext::Cell | TableContext::RowHeader) {
            if let Some(row_h) = Self::uniform_row_height_near_viewport(table, start_row, rows) {
                if let Some((_, start_y, _, _)) = table.find_cell(context, start_row, anchor_col) {
                    if let Some(candidate) = Self::estimate_uniform_row_candidate(
                        start_row, start_y, row_h, mouse_y, rows,
                    ) {
                        let (_, candidate_y, _, candidate_h) =
                            table.find_cell(context, candidate, anchor_col)?;
                        if candidate_h > 0
                            && mouse_y >= candidate_y
                            && mouse_y < candidate_y.saturating_add(candidate_h)
                        {
                            return Some(candidate);
                        }
                    }
                }
            }
        }

        let (_, start_y, _, row_h) = table.find_cell(context, start_row, anchor_col)?;
        if row_h <= 0 {
            return None;
        }

        let delta = mouse_y.saturating_sub(start_y);
        let mut candidate = start_row.saturating_add(safe_div(delta, row_h));
        let last_row = rows.saturating_sub(1);
        candidate = candidate.max(0).min(last_row);

        let (_, candidate_y, _, candidate_h) = table.find_cell(context, candidate, anchor_col)?;
        if candidate_h > 0
            && mouse_y >= candidate_y
            && mouse_y < candidate_y.saturating_add(candidate_h)
        {
            Some(candidate)
        } else {
            None
        }
    }

    fn estimate_col_hit(
        table: &Table,
        context: TableContext,
        anchor_row: i32,
        start_col: i32,
        mouse_x: i32,
        cols: i32,
    ) -> Option<i32> {
        if anchor_row < 0 || start_col < 0 || cols <= 0 || start_col >= cols {
            return None;
        }

        let (start_x, _, start_w, _) = table.find_cell(context, anchor_row, start_col)?;
        if start_w <= 0 {
            return None;
        }

        let delta = mouse_x.saturating_sub(start_x);
        let mut candidate = start_col.saturating_add(safe_div(delta, start_w));
        let last_col = cols.saturating_sub(1);
        candidate = candidate.max(0).min(last_col);

        let (candidate_x, _, candidate_w, _) = table.find_cell(context, anchor_row, candidate)?;
        if candidate_w > 0
            && mouse_x >= candidate_x
            && mouse_x < candidate_x.saturating_add(candidate_w)
        {
            Some(candidate)
        } else {
            None
        }
    }

    fn pointer_moved_beyond_tolerance(
        start_x: i32,
        start_y: i32,
        current_x: i32,
        current_y: i32,
        tolerance_px: u32,
    ) -> bool {
        start_x.abs_diff(current_x) > tolerance_px || start_y.abs_diff(current_y) > tolerance_px
    }

    /// Returns `true` when the mouse position falls inside one of the FLTK
    /// Table's embedded scrollbar widgets (vertical or horizontal).
    fn is_mouse_on_table_scrollbar(table: &Table, mouse_x: i32, mouse_y: i32) -> bool {
        if Self::is_mouse_on_vertical_table_scrollbar(table, mouse_x, mouse_y) {
            return true;
        }
        // Check horizontal scrollbar.
        let hsb = table.hscrollbar();
        if hsb.visible() {
            let hx = hsb.x();
            let hy = hsb.y();
            if mouse_x >= hx && mouse_x < hx + hsb.w() && mouse_y >= hy && mouse_y < hy + hsb.h() {
                return true;
            }
        }
        false
    }

    fn is_mouse_on_vertical_table_scrollbar(table: &Table, mouse_x: i32, mouse_y: i32) -> bool {
        let vsb = table.scrollbar();
        if vsb.visible() {
            let vx = vsb.x();
            let vy = vsb.y();
            if mouse_x >= vx && mouse_x < vx + vsb.w() && mouse_y >= vy && mouse_y < vy + vsb.h() {
                return true;
            }
        }
        false
    }

    /// Get cell at mouse position (returns None if outside cells)
    fn get_cell_at_mouse(table: &Table) -> Option<(i32, i32)> {
        let rows = table.rows();
        let cols = table.cols();
        if rows <= 0 || cols <= 0 {
            return None;
        }

        let mouse_x = app::event_x();
        let mouse_y = app::event_y();

        let table_x = table.x();
        let table_y = table.y();
        let table_w = table.w();
        let table_h = table.h();
        let data_left = table_x + table.row_header_width();
        let data_top = table_y + table.col_header_height();
        let data_right = table_x + table_w;
        let data_bottom = table_y + table_h;

        if mouse_x < data_left
            || mouse_y < data_top
            || mouse_x >= data_right
            || mouse_y >= data_bottom
        {
            return None;
        }

        // Exclude clicks on FLTK Table's embedded scrollbar widgets.
        // Scrollbars are child widgets of the Table group; if the mouse lands
        // on a visible scrollbar we must return None so the default handler
        // can process the scroll action instead of selecting a cell.
        if Self::is_mouse_on_table_scrollbar(table, mouse_x, mouse_y) {
            return None;
        }

        let last_row = rows.saturating_sub(1);
        let last_col = cols.saturating_sub(1);
        let start_row = table.row_position().max(0).min(last_row);
        let start_col = table.col_position().max(0).min(last_col);

        // Ignore clicks on scrollbar gutters (especially bottom horizontal scrollbar).
        // When the mouse is outside the currently visible cell viewport, the fallback scan
        // below can become expensive near the last row because it walks from 0..start_row.
        if let Some((visible_right, visible_bottom)) =
            Self::visible_cell_bounds(table, start_row, start_col)
        {
            if mouse_x >= visible_right || mouse_y >= visible_bottom {
                return None;
            }
        }

        let mut row_hit = Self::estimate_row_hit(
            table,
            TableContext::Cell,
            start_row,
            start_col,
            mouse_y,
            rows,
        );
        if row_hit.is_none() {
            let mut row = start_row;
            while row < rows {
                if let Some((_, cy, _, ch)) = table.find_cell(TableContext::Cell, row, start_col) {
                    if mouse_y >= cy && mouse_y < cy.saturating_add(ch) {
                        row_hit = Some(row);
                        break;
                    }
                    if cy > mouse_y || cy >= data_bottom {
                        break;
                    }

                    // When row_position is temporarily stale, FLTK can return many
                    // off-screen rows before reaching the visible viewport. Use the
                    // observed row height to skip in larger steps.
                    if let Some(skip) = Self::offscreen_skip_count(cy, ch, data_top) {
                        row = row.saturating_add(skip);
                        continue;
                    }
                } else {
                    break;
                }
                row += 1;
            }
        }

        let row_hit = match row_hit {
            Some(row_hit) => row_hit,
            None => {
                // set_row_position() 직후에는 FLTK 내부 row_position 값이
                // 실제 렌더링 viewport와 잠시 어긋나는 경우가 있어,
                // start_row 이후만 스캔하면 셀 hit-test가 실패할 수 있다.
                // (사용자가 스크롤을 한 번 더 움직이면 정상화되는 현상)
                let mut row = Self::fallback_scan_start(start_row, MAX_HITTEST_ROW_BACKTRACK);
                while row < start_row {
                    if let Some((_, cy, _, ch)) = table.find_cell(TableContext::Cell, row, 0) {
                        if mouse_y >= cy && mouse_y < cy.saturating_add(ch) {
                            row_hit = Some(row);
                            break;
                        }
                        if cy > mouse_y || cy >= data_bottom {
                            break;
                        }
                    }
                    row += 1;
                }

                row_hit?
            }
        };

        let scan_start_col = Self::skip_hidden_columns(table, row_hit, start_col, data_left, cols);
        let mut col = scan_start_col;
        while col < cols {
            if let Some((cx, _, cw, _)) = table.find_cell(TableContext::Cell, row_hit, col) {
                if mouse_x >= cx && mouse_x < cx + cw {
                    return Some((row_hit, col));
                }
                if cx > mouse_x || cx >= data_right {
                    break;
                }

                // Skip multiple off-screen / already-passed columns at once to
                // avoid O(total_columns) scans during drag-selection.
                if let Some(skip) = Self::offscreen_skip_count(cx, cw, mouse_x) {
                    col = col.saturating_add(skip);
                    continue;
                }
            } else {
                break;
            }
            col += 1;
        }

        // The skip above can overshoot when a narrow column precedes a wide one.
        // Scan backward from where the forward scan ended to catch skipped columns.
        if col > scan_start_col + 1 {
            let mut back = col.saturating_sub(1).min(last_col);
            loop {
                if back <= scan_start_col {
                    break;
                }
                if let Some((cx, _, cw, _)) = table.find_cell(TableContext::Cell, row_hit, back) {
                    if mouse_x >= cx && mouse_x < cx + cw {
                        return Some((row_hit, back));
                    }
                    if cw > 0 && cx + cw <= mouse_x {
                        break;
                    }
                } else {
                    break;
                }
                back -= 1;
            }
        }

        None
    }

    fn resolve_double_click_target_cell(table: &Table) -> Option<(i32, i32)> {
        Self::native_selected_cell_after_push(table)
    }

    fn native_selected_cell_after_push(table: &Table) -> Option<(i32, i32)> {
        Self::single_cell_from_selection(
            table.get_selection(),
            table.rows().max(0) as usize,
            table.cols().max(0) as usize,
        )
    }

    fn single_cell_from_selection(
        selection: (i32, i32, i32, i32),
        max_rows: usize,
        max_cols: usize,
    ) -> Option<(i32, i32)> {
        let (row, col) = Self::resolve_update_target_cell(selection, max_rows, max_cols, None)?;
        Some((i32::try_from(row).ok()?, i32::try_from(col).ok()?))
    }

    fn visible_cell_bounds(table: &Table, start_row: i32, start_col: i32) -> Option<(i32, i32)> {
        if start_row < 0 || start_col < 0 {
            return None;
        }

        let rows = table.rows();
        let cols = table.cols();
        if start_row >= rows || start_col >= cols {
            return None;
        }

        let data_bottom = table.y() + table.h();
        let data_right = table.x() + table.w();
        let data_top = table.y() + table.col_header_height();

        let mut visible_bottom: Option<i32> = None;
        if let Some(row_h) = Self::uniform_row_height_near_viewport(table, start_row, rows) {
            if let Some((_, bottom_px)) = Self::visible_row_metrics(data_top, data_bottom, row_h) {
                visible_bottom = Some(bottom_px);
            }
        } else {
            let mut row = start_row;
            while row < rows {
                let Some((_, cy, _, ch)) = table.find_cell(TableContext::Cell, row, start_col)
                else {
                    break;
                };
                if let Some(skip) = Self::offscreen_skip_count(cy, ch, data_top) {
                    row = row.saturating_add(skip);
                    continue;
                }
                let row_bottom = cy + ch;
                visible_bottom =
                    Some(visible_bottom.map_or(row_bottom, |prev| prev.max(row_bottom)));
                if row_bottom >= data_bottom {
                    break;
                }
                row += 1;
            }
        }

        let mut visible_right: Option<i32> = None;
        let mut col = Self::skip_hidden_columns(
            table,
            start_row,
            start_col,
            table.x() + table.row_header_width(),
            cols,
        );
        while col < cols {
            let Some((cx, _, cw, _)) = table.find_cell(TableContext::Cell, start_row, col) else {
                break;
            };
            let col_right = cx + cw;
            visible_right = Some(visible_right.map_or(col_right, |prev| prev.max(col_right)));
            if col_right >= data_right {
                break;
            }
            col += 1;
        }

        match (visible_right, visible_bottom) {
            (Some(right), Some(bottom)) => Some((right, bottom)),
            _ => None,
        }
    }

    /// Get row index when mouse is over row header area.
    fn get_row_header_at_mouse(table: &Table) -> Option<i32> {
        let rows = table.rows();
        if rows <= 0 {
            return None;
        }

        let mouse_x = app::event_x();
        let mouse_y = app::event_y();

        let table_x = table.x();
        let table_y = table.y();
        let table_h = table.h();
        let row_header_right = table_x + table.row_header_width();
        let data_top = table_y + table.col_header_height();
        let data_bottom = table_y + table_h;

        if mouse_x < table_x
            || mouse_x >= row_header_right
            || mouse_y < data_top
            || mouse_y >= data_bottom
        {
            return None;
        }

        let last_row = rows.saturating_sub(1);
        let start_row = table.row_position().max(0).min(last_row);
        if let Some(row) =
            Self::estimate_row_hit(table, TableContext::RowHeader, start_row, 0, mouse_y, rows)
        {
            return Some(row);
        }

        // get_cell_at_mouse와 동일하게 row_position stale 케이스를 보완한다.
        let mut row = Self::fallback_scan_start(start_row, MAX_HITTEST_ROW_BACKTRACK);
        while row < start_row {
            if let Some((_, cy, _, ch)) = table.find_cell(TableContext::RowHeader, row, 0) {
                if mouse_y >= cy && mouse_y < cy.saturating_add(ch) {
                    return Some(row);
                }
                if cy > mouse_y || cy >= data_bottom {
                    break;
                }
            }
            row += 1;
        }

        None
    }

    fn get_col_header_at_mouse(table: &Table) -> Option<i32> {
        let cols = table.cols();
        if cols <= 0 {
            return None;
        }

        let mouse_x = app::event_x();
        let mouse_y = app::event_y();

        let table_x = table.x();
        let table_y = table.y();
        let table_w = table.w();
        let data_left = table_x + table.row_header_width();
        let data_right = table_x + table_w;
        let header_bottom = table_y + table.col_header_height();

        if mouse_x < data_left
            || mouse_x >= data_right
            || mouse_y < table_y
            || mouse_y >= header_bottom
        {
            return None;
        }

        let rows = table.rows();
        let anchor_row = if rows > 0 {
            let last_row = rows.saturating_sub(1);
            table.row_position().max(0).min(last_row)
        } else {
            0
        };
        let last_col = cols.saturating_sub(1);
        let start_col = table.col_position().max(0).min(last_col);

        if let Some(col) = Self::estimate_col_hit(
            table,
            TableContext::ColHeader,
            anchor_row,
            start_col,
            mouse_x,
            cols,
        ) {
            return Some(col);
        }

        let mut col = start_col;
        while col < cols {
            if let Some((cx, _, cw, _)) = table.find_cell(TableContext::ColHeader, anchor_row, col)
            {
                if cw > 0 && mouse_x >= cx && mouse_x < cx.saturating_add(cw) {
                    return Some(col);
                }
                if cx > mouse_x || cx >= data_right {
                    break;
                }
                if cw > 0 && cx.saturating_add(cw) <= mouse_x {
                    let gap_px = mouse_x - cx.saturating_add(cw);
                    let skip = safe_div(gap_px, cw).max(1);
                    col = col.saturating_add(skip);
                    continue;
                }
            } else {
                break;
            }
            col += 1;
        }

        // The skip optimisation above uses the current column's width to estimate
        // how many columns to jump.  When a narrow column precedes a wide one the
        // skip can overshoot the target.  Scan backward from the position where the
        // forward scan ended to catch any column that was skipped over.
        if col > start_col + 1 {
            let mut back = col.saturating_sub(1).min(last_col);
            loop {
                if back <= start_col {
                    break;
                }
                if let Some((cx, _, cw, _)) =
                    table.find_cell(TableContext::ColHeader, anchor_row, back)
                {
                    if cw > 0 && mouse_x >= cx && mouse_x < cx.saturating_add(cw) {
                        return Some(back);
                    }
                    // Mouse is to the right of this column's right edge; columns
                    // further left are even further away – stop searching.
                    if cw > 0 && cx.saturating_add(cw) <= mouse_x {
                        break;
                    }
                    // cw == 0 (hidden column) or column is to the right of mouse:
                    // continue scanning toward lower indices.
                } else {
                    break;
                }
                back -= 1;
            }
        }

        col = Self::fallback_scan_start(start_col, MAX_HITTEST_COL_BACKTRACK);
        while col < start_col {
            if let Some((cx, _, cw, _)) = table.find_cell(TableContext::ColHeader, anchor_row, col)
            {
                if cw > 0 && mouse_x >= cx && mouse_x < cx.saturating_add(cw) {
                    return Some(col);
                }
                if cx > mouse_x || cx >= data_right {
                    break;
                }
            }
            col += 1;
        }

        None
    }

    fn has_visible_table_columns(table: &Table, hidden_col: Option<usize>) -> bool {
        let cols = table.cols().max(0) as usize;
        (0..cols).any(|col| Some(col) != hidden_col)
    }

    fn is_mouse_inside_table_body(table: &Table, mouse_x: i32, mouse_y: i32) -> bool {
        let data_left = table.x() + table.row_header_width();
        let data_top = table.y() + table.col_header_height();
        mouse_x >= data_left
            && mouse_x < table.x() + table.w()
            && mouse_y >= data_top
            && mouse_y < table.y() + table.h()
    }

    fn should_show_context_menu_for_hit(
        clicked_cell: Option<(i32, i32)>,
        clicked_row_header: Option<i32>,
        clicked_col_header: Option<i32>,
        clicked_empty_body_with_columns: bool,
    ) -> bool {
        clicked_cell.is_some()
            || clicked_row_header.is_some()
            || clicked_col_header.is_some()
            || clicked_empty_body_with_columns
    }

    fn skip_hidden_columns(
        table: &Table,
        row: i32,
        start_col: i32,
        data_left: i32,
        cols: i32,
    ) -> i32 {
        if row < 0 || start_col < 0 || cols <= 0 || start_col >= cols {
            return start_col;
        }

        let mut col = start_col;
        while col < cols {
            let Some((cx, _, cw, _)) = table.find_cell(TableContext::Cell, row, col) else {
                break;
            };

            if cx + cw > data_left || cw <= 0 {
                break;
            }

            let hidden_px = data_left - (cx + cw);
            let skip = safe_div(hidden_px, cw).max(1);
            col = col.saturating_add(skip);
        }

        col.min(cols.saturating_sub(1))
    }

    fn is_unquoted_identifier(text: &str) -> bool {
        let mut chars = text.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !(first.is_ascii_alphabetic() || matches!(first, '_' | '$' | '#')) {
            return false;
        }
        chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '#'))
    }

    fn quote_identifier_segment(text: &str) -> String {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return "\"\"".to_string();
        }
        if trimmed.starts_with('"') && trimmed.ends_with('"') {
            return trimmed.to_string();
        }
        if crate::sql_text::is_quoted_identifier(trimmed) {
            let unquoted = crate::sql_text::strip_identifier_quotes(trimmed);
            return format!("\"{}\"", unquoted.replace('"', "\"\""));
        }
        // Keep unquoted identifiers unquoted regardless of case so SQL semantics
        // (case-insensitive resolution to upper identifiers) are preserved.
        if Self::is_unquoted_identifier(trimmed) {
            return trimmed.to_string();
        }
        format!("\"{}\"", trimmed.replace('"', "\"\""))
    }

    fn split_qualified_identifier(name: &str) -> Vec<&str> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }

        let mut segments = Vec::new();
        let mut active_quote: Option<char> = None;
        let mut segment_start = 0usize;
        let mut chars = trimmed.char_indices().peekable();
        while let Some((idx, ch)) = chars.next() {
            if let Some(delimiter) = active_quote {
                if ch == delimiter {
                    if chars
                        .peek()
                        .is_some_and(|(_, next_ch)| *next_ch == delimiter)
                    {
                        chars.next();
                    } else {
                        active_quote = None;
                    }
                }
                continue;
            }

            match ch {
                '"' | '`' => active_quote = Some(ch),
                '[' => active_quote = Some(']'),
                '.' => {
                    if trimmed.is_char_boundary(segment_start) && trimmed.is_char_boundary(idx) {
                        let segment = trimmed[segment_start..idx].trim();
                        if !segment.is_empty() {
                            segments.push(segment);
                        }
                    }
                    segment_start = idx + ch.len_utf8();
                }
                _ => {}
            }
        }

        if trimmed.is_char_boundary(segment_start) {
            let segment = trimmed[segment_start..].trim();
            if !segment.is_empty() {
                segments.push(segment);
            }
        }
        segments
    }

    fn quote_qualified_identifier(name: &str) -> String {
        let segments = Self::split_qualified_identifier(name);
        if segments.is_empty() {
            return Self::quote_identifier_segment(name);
        }
        segments
            .into_iter()
            .map(Self::quote_identifier_segment)
            .collect::<Vec<_>>()
            .join(".")
    }

    fn last_identifier_segment(name: &str) -> &str {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return "";
        }

        let mut active_quote: Option<char> = None;
        let mut last_dot_idx = None;
        let mut chars = trimmed.char_indices().peekable();
        while let Some((idx, ch)) = chars.next() {
            if let Some(delimiter) = active_quote {
                if ch == delimiter {
                    if chars
                        .peek()
                        .is_some_and(|(_, next_ch)| *next_ch == delimiter)
                    {
                        chars.next();
                    } else {
                        active_quote = None;
                    }
                }
                continue;
            }

            match ch {
                '"' | '`' => active_quote = Some(ch),
                '[' => active_quote = Some(']'),
                '.' => last_dot_idx = Some(idx),
                _ => {}
            }
        }

        if let Some(dot_idx) = last_dot_idx {
            let start = dot_idx + 1;
            if trimmed.is_char_boundary(start) {
                return trimmed[start..].trim();
            }
        }
        trimmed
    }

    fn editable_column_identifier(column_header: &str) -> Option<String> {
        let column = Self::last_identifier_segment(column_header);
        if column.is_empty() {
            return None;
        }
        if !Self::is_valid_identifier_segment(column) {
            return None;
        }
        Some(Self::quote_identifier_segment(column))
    }

    fn is_valid_identifier_segment(segment: &str) -> bool {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            return false;
        }

        if crate::sql_text::is_quoted_identifier(trimmed) {
            let inner = crate::sql_text::strip_identifier_quotes(trimmed);
            if inner.is_empty() {
                return false;
            }
            return true;
        }

        let mut chars = trimmed.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !(first == '_' || first.is_ascii_alphabetic()) {
            return false;
        }

        chars.all(|ch| ch == '_' || ch == '$' || ch == '#' || ch.is_ascii_alphanumeric())
    }

    fn sql_string_literal(value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }

    fn validate_sql_expression_input(expr: &str) -> Result<String, String> {
        crate::ui::sql_editor::query_text::validate_sql_expression_input(expr)
    }

    fn sql_literal_from_input_with_null_text(
        input: &str,
        null_text: &str,
    ) -> Result<String, String> {
        let trimmed = input.trim();
        if input.is_empty() || Self::input_matches_null_text(trimmed, null_text) {
            return Ok("NULL".to_string());
        }
        if let Some(expr) = trimmed.strip_prefix('=') {
            return Self::validate_sql_expression_input(expr);
        }
        // Treat non-expression user input as a string literal.
        //
        // Previous behavior auto-detected numeric-looking values (e.g. "00123")
        // and emitted them as numeric SQL literals. On character columns this can
        // cause implicit conversion and silently lose formatting ("00123" -> "123").
        // Users can still force a numeric/expression assignment explicitly with
        // the documented '=expr' syntax.
        // Preserve user-entered leading/trailing whitespace for string literals.
        Ok(Self::sql_string_literal(input))
    }

    #[cfg(all(test, not(target_os = "linux")))]
    fn sql_literal_from_input(input: &str) -> Result<String, String> {
        Self::sql_literal_from_input_with_null_text(input, "NULL")
    }

    #[allow(dead_code)]
    fn compose_edit_script(dml_sql: &str, source_sql: &str) -> String {
        let dml = dml_sql.trim().trim_end_matches(';').trim();
        let select_sql = source_sql.trim().trim_end_matches(';').trim();
        if dml.is_empty() {
            return String::new();
        }
        if select_sql.is_empty() {
            return dml.to_string();
        }
        format!("{dml};\n{select_sql}")
    }

    fn canonical_sql_signature(sql: &str) -> String {
        let token_spans = crate::ui::sql_editor::query_text::tokenize_sql_spanned(sql);
        let mut normalized = String::with_capacity(sql.len());
        let mut previous_join_class: Option<CanonicalJoinClass> = None;

        for span in token_spans {
            let token_text = match span.token {
                crate::ui::sql_editor::SqlToken::Comment(_) => continue,
                crate::ui::sql_editor::SqlToken::Word(word) => {
                    if Self::is_quoted_identifier_token(&word) {
                        word
                    } else {
                        word.to_ascii_uppercase()
                    }
                }
                crate::ui::sql_editor::SqlToken::String(text) => text,
                crate::ui::sql_editor::SqlToken::Symbol(symbol) => symbol,
            };

            let join_class = Self::canonical_join_class(&token_text);
            if previous_join_class
                .zip(Some(join_class))
                .is_some_and(|(left, right)| {
                    Self::needs_space_between_canonical_tokens(left, right)
                })
            {
                normalized.push(' ');
            }

            normalized.push_str(&token_text);
            previous_join_class = Some(join_class);
        }

        normalized.trim_end_matches(';').trim().to_string()
    }

    fn is_quoted_identifier_token(word: &str) -> bool {
        word.starts_with('"') && word.ends_with('"')
    }

    fn canonical_join_class(token_text: &str) -> CanonicalJoinClass {
        if token_text == "." {
            return CanonicalJoinClass::Dot;
        }

        if token_text.chars().all(Self::is_canonical_word_char) {
            return CanonicalJoinClass::WordLike;
        }

        CanonicalJoinClass::Symbolic
    }

    fn is_canonical_word_char(ch: char) -> bool {
        ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '#')
    }

    fn needs_space_between_canonical_tokens(
        left: CanonicalJoinClass,
        right: CanonicalJoinClass,
    ) -> bool {
        matches!(
            (left, right),
            (CanonicalJoinClass::WordLike, CanonicalJoinClass::WordLike)
                | (CanonicalJoinClass::WordLike, CanonicalJoinClass::Symbolic)
                | (CanonicalJoinClass::Symbolic, CanonicalJoinClass::WordLike)
        )
    }

    fn matches_pending_save_signature(pending_signature: Option<&str>, result_sql: &str) -> bool {
        let result_signature = Self::canonical_sql_signature(result_sql);
        pending_signature
            .map(|signature| signature == result_signature)
            .unwrap_or(false)
    }

    fn matches_pending_save_tag(pending_tag: Option<&str>, result_sql: &str) -> bool {
        let Some(tag) = pending_tag else {
            return false;
        };
        Self::contains_tag_token(result_sql, tag)
    }

    fn matches_pending_save_tag_in_message(pending_tag: Option<&str>, message: &str) -> bool {
        let Some(tag) = pending_tag else {
            return false;
        };
        Self::contains_tag_token(message, tag)
    }

    fn contains_tag_token(haystack: &str, tag: &str) -> bool {
        if tag.is_empty() {
            return false;
        }
        haystack.match_indices(tag).any(|(start, _)| {
            let end = start.saturating_add(tag.len());
            let prev_ok = haystack
                .get(..start)
                .and_then(|prefix| prefix.chars().next_back())
                .map(|ch| !Self::is_save_tag_identifier_char(ch))
                .unwrap_or(true);
            let next_ok = haystack
                .get(end..)
                .and_then(|suffix| suffix.chars().next())
                .map(|ch| !Self::is_save_tag_identifier_char(ch))
                .unwrap_or(true);
            prev_ok && next_ok
        })
    }

    fn is_save_tag_identifier_char(ch: char) -> bool {
        ch.is_ascii_alphanumeric() || ch == '_' || ch == ':'
    }

    fn is_pending_save_terminal_result(
        pending_tag: Option<&str>,
        pending_signature: Option<&str>,
        pending_statement_signatures: &[String],
        result: &QueryResult,
    ) -> bool {
        if Self::matches_pending_save_tag(pending_tag, &result.sql)
            || Self::matches_pending_save_tag_in_message(pending_tag, &result.message)
            || Self::matches_pending_save_signature(pending_signature, &result.sql)
        {
            return true;
        }

        if !result.is_select && !result.sql.trim().is_empty() {
            let result_signature = Self::canonical_sql_signature(&result.sql);
            if !result_signature.is_empty()
                && pending_statement_signatures
                    .iter()
                    .any(|signature| signature == &result_signature)
            {
                return true;
            }
        }

        // Some terminal packets can lose statement SQL text. Keep fallback
        // strict (non-select + empty SQL + active save tracking metadata) so
        // unrelated out-of-order results do not accidentally clear save-pending.
        let has_tracking_metadata = pending_tag.is_some() || pending_signature.is_some();
        has_tracking_metadata
            && !result.is_select
            && result.sql.trim().is_empty()
            && ((!result.success
                && (Self::is_execution_abort_message(&result.message)
                    || Self::is_connection_loss_message(&result.message)))
                || (result.success
                    && Self::is_non_select_success_completion_message(&result.message)))
    }

    // Message classification is DB-agnostic here (the result table does not
    // know the originating db_type), so it delegates to the shared per-DB
    // marker catalogs in `db::session_policy`.
    fn is_execution_abort_message(message: &str) -> bool {
        crate::db::session_policy::message_indicates_execution_abort(message)
    }

    fn is_query_cancel_message(message: &str) -> bool {
        crate::db::session_policy::message_indicates_query_cancel(message)
    }

    fn is_connection_loss_message(message: &str) -> bool {
        crate::db::session_policy::message_indicates_connection_loss(message)
    }

    fn is_non_select_success_completion_message(message: &str) -> bool {
        let lowered = message.trim().to_ascii_lowercase();
        [
            result_messages::ROWS_AFFECTED_FRAGMENT,
            result_messages::STATEMENT_EXECUTED,
            result_messages::COMMIT_COMPLETE,
            result_messages::ROLLBACK_COMPLETE,
            result_messages::PLSQL_BLOCK_EXECUTED,
            result_messages::CALL_EXECUTED,
            result_messages::AUTO_COMMIT_APPLIED,
        ]
        .iter()
        .any(|fragment| lowered.contains(&fragment.to_ascii_lowercase()))
    }

    fn normalize_header_for_lookup(header: &str) -> String {
        header.replace('"', "").trim().to_ascii_uppercase()
    }

    fn find_rowid_column_index(headers: &[String]) -> Option<usize> {
        headers.iter().position(|name| {
            let normalized = Self::normalize_header_for_lookup(name);
            normalized == "ROWID" || normalized.ends_with(".ROWID")
        })
    }

    fn detect_auto_hidden_rowid_col(
        headers: &[String],
        _source_sql: &str,
        edit_mode_enabled: bool,
    ) -> Option<usize> {
        if edit_mode_enabled {
            return None;
        }
        // Keep ROWID hidden while edit mode is disabled so streaming rows do not
        // briefly expose the technical column before the source SQL is set.
        let rowid_col = Self::find_rowid_column_index(headers)?;
        if rowid_col != 0 {
            return None;
        }
        Some(rowid_col)
    }

    fn hidden_auto_rowid_col_value(&self) -> Option<usize> {
        *self
            .hidden_auto_rowid_col
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn apply_hidden_rowid_column_width(&mut self) {
        let Some(hidden_col) = self.hidden_auto_rowid_col_value() else {
            return;
        };
        if hidden_col >= self.table.cols().max(0) as usize {
            return;
        }
        self.table.set_col_width(hidden_col as i32, 0);
    }

    fn refresh_table_layout_geometry(&mut self) {
        // Force FLTK to recompute scroll range and visible viewport
        // after runtime column width changes.
        let (x, y, w, h) = (
            self.table.x(),
            self.table.y(),
            self.table.w(),
            self.table.h(),
        );
        self.table.resize(x, y, w, h);
    }

    fn sync_table_viewport_state(&mut self) {
        self.refresh_table_layout_geometry();

        let rows = self.table.rows().max(0);
        if rows > 0 {
            let last_row = rows - 1;
            let current_row = self.table.row_position().max(0).min(last_row);
            self.table.set_row_position(current_row);
        } else {
            self.table.set_row_position(0);
        }

        let cols = self.table.cols().max(0);
        if cols > 0 {
            let last_col = cols - 1;
            let current_col = self.table.col_position().max(0).min(last_col);
            self.table.set_col_position(current_col);
        } else {
            self.table.set_col_position(0);
        }

        self.table.redraw();
        app::redraw();
    }

    fn refresh_auto_rowid_visibility(&mut self) {
        let headers_snapshot = self
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let source_sql = self
            .source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let edit_mode_enabled = self.is_edit_mode_enabled();
        let next_hidden_col =
            Self::detect_auto_hidden_rowid_col(&headers_snapshot, &source_sql, edit_mode_enabled);
        let previous_hidden_col = self.hidden_auto_rowid_col_value();
        if previous_hidden_col == next_hidden_col {
            self.apply_hidden_rowid_column_width();
            Self::clamp_selection_to_visible_columns(&mut self.table, next_hidden_col);
            self.sync_table_viewport_state();
            return;
        }

        *self
            .hidden_auto_rowid_col
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = next_hidden_col;

        if previous_hidden_col.is_some() && next_hidden_col.is_none() {
            self.recalculate_widths_for_current_font();
        }
        self.apply_hidden_rowid_column_width();
        Self::clamp_selection_to_visible_columns(&mut self.table, next_hidden_col);
        self.sync_table_viewport_state();
    }

    fn visible_column_indices_in_range(
        col_left: usize,
        col_right: usize,
        hidden_col: Option<usize>,
    ) -> Vec<usize> {
        (col_left..=col_right)
            .filter(|col| Some(*col) != hidden_col)
            .collect()
    }

    fn visible_headers(headers: &[String], hidden_col: Option<usize>) -> Vec<String> {
        headers
            .iter()
            .enumerate()
            .filter(|(idx, _)| Some(*idx) != hidden_col)
            .map(|(_, header)| header.clone())
            .collect()
    }

    fn visible_row_values_internal(row: &[String], hidden_col: Option<usize>) -> Vec<String> {
        row.iter()
            .enumerate()
            .filter(|(idx, _)| Some(*idx) != hidden_col)
            .map(|(_, value)| value.clone())
            .collect()
    }

    fn resolve_target_table(source_sql: &str) -> Result<String, String> {
        crate::ui::sql_editor::query_text::resolve_edit_target_table(source_sql)
    }

    fn selected_anchor_cell(table: &Table) -> Option<(usize, usize)> {
        let (row_start, col_start, _, _) = Self::normalized_selection_bounds_with_limits(
            table.get_selection(),
            table.rows().max(0) as usize,
            table.cols().max(0) as usize,
        )?;
        Some((row_start, col_start))
    }

    #[allow(dead_code)]
    fn selected_row(table: &Table) -> Option<usize> {
        Self::selected_anchor_cell(table).map(|(row, _)| row)
    }

    fn selected_row_range(table: &Table) -> Option<(usize, usize)> {
        let (row_start, _, row_end, _) = Self::normalized_selection_bounds_with_limits(
            table.get_selection(),
            table.rows().max(0) as usize,
            table.cols().max(0) as usize,
        )?;
        Some((row_start, row_end))
    }

    fn normalized_selection_bounds(
        selection: (i32, i32, i32, i32),
    ) -> Option<(usize, usize, usize, usize)> {
        let (row_top, col_left, row_bot, col_right) = selection;
        if row_top < 0 || col_left < 0 || row_bot < 0 || col_right < 0 {
            return None;
        }

        let row_start = row_top.min(row_bot) as usize;
        let row_end = row_top.max(row_bot) as usize;
        let col_start = col_left.min(col_right) as usize;
        let col_end = col_left.max(col_right) as usize;
        Some((row_start, col_start, row_end, col_end))
    }

    fn normalized_selection_bounds_with_limits(
        selection: (i32, i32, i32, i32),
        max_rows: usize,
        max_cols: usize,
    ) -> Option<(usize, usize, usize, usize)> {
        if max_rows == 0 || max_cols == 0 {
            return None;
        }

        let (row_start, col_start, row_end, col_end) =
            Self::normalized_selection_bounds(selection)?;
        if row_start >= max_rows || col_start >= max_cols {
            return None;
        }

        let row_max = max_rows.saturating_sub(1);
        let col_max = max_cols.saturating_sub(1);
        let row_start = row_start.min(row_max);
        let row_end = row_end.min(row_max);
        let col_start = col_start.min(col_max);
        let col_end = col_end.min(col_max);

        if row_start > row_end || col_start > col_end {
            None
        } else {
            Some((row_start, col_start, row_end, col_end))
        }
    }

    fn selection_contains_cell(selection: (i32, i32, i32, i32), row: i32, col: i32) -> bool {
        if row < 0 || col < 0 {
            return false;
        }
        let Some((row_start, col_start, row_end, col_end)) =
            Self::normalized_selection_bounds(selection)
        else {
            return false;
        };

        let row = row as usize;
        let col = col as usize;
        row >= row_start && row <= row_end && col >= col_start && col <= col_end
    }

    fn oriented_selection_with_cell(
        base_selection: Option<(i32, i32, i32, i32)>,
        row: i32,
        col: i32,
        max_rows: usize,
        max_cols: usize,
    ) -> Option<(i32, i32, i32, i32)> {
        if row < 0 || col < 0 || max_rows == 0 || max_cols == 0 {
            return None;
        }

        let row = usize::try_from(row).ok()?;
        let col = usize::try_from(col).ok()?;
        if row >= max_rows || col >= max_cols {
            return None;
        }

        let (anchor_row, anchor_col) = base_selection
            .and_then(|selection| {
                Self::oriented_selection_with_limits(selection, max_rows, max_cols)
            })
            .map(|(anchor_row, anchor_col, _, _)| (anchor_row, anchor_col))
            .unwrap_or((row, col));

        Some((
            i32::try_from(anchor_row).ok()?,
            i32::try_from(anchor_col).ok()?,
            i32::try_from(row).ok()?,
            i32::try_from(col).ok()?,
        ))
    }

    fn shift_click_selection_with_cell(
        base_selection: Option<(i32, i32, i32, i32)>,
        row: i32,
        col: i32,
        max_rows: usize,
        max_cols: usize,
    ) -> Option<(i32, i32, i32, i32)> {
        let Some(base_selection) = base_selection else {
            return Self::oriented_selection_with_cell(None, row, col, max_rows, max_cols);
        };

        let base_bounds =
            Self::normalized_selection_bounds_with_limits(base_selection, max_rows, max_cols)?;
        if base_bounds.0 == base_bounds.2 && base_bounds.1 == base_bounds.3 {
            return Self::oriented_selection_with_cell(
                Some(base_selection),
                row,
                col,
                max_rows,
                max_cols,
            );
        }

        Self::expanded_selection_bounds_with_cell(Some(base_bounds), row, col, max_rows, max_cols)
    }

    fn expanded_selection_bounds_with_cell(
        base_bounds: Option<(usize, usize, usize, usize)>,
        row: i32,
        col: i32,
        max_rows: usize,
        max_cols: usize,
    ) -> Option<(i32, i32, i32, i32)> {
        if row < 0 || col < 0 || max_rows == 0 || max_cols == 0 {
            return None;
        }

        let row = usize::try_from(row).ok()?;
        let col = usize::try_from(col).ok()?;
        if row >= max_rows || col >= max_cols {
            return None;
        }

        let (row_start, col_start, row_end, col_end) = base_bounds.unwrap_or((row, col, row, col));
        Some((
            i32::try_from(row_start.min(row)).ok()?,
            i32::try_from(col_start.min(col)).ok()?,
            i32::try_from(row_end.max(row)).ok()?,
            i32::try_from(col_end.max(col)).ok()?,
        ))
    }

    fn resolve_update_target_cell(
        selection: (i32, i32, i32, i32),
        max_rows: usize,
        max_cols: usize,
        context_cell: Option<(usize, usize)>,
    ) -> Option<(usize, usize)> {
        if let Some((row, col)) = context_cell {
            if row >= max_rows || col >= max_cols {
                return None;
            }
            return Some((row, col));
        }

        let (row_start, col_start, row_end, col_end) =
            Self::normalized_selection_bounds_with_limits(selection, max_rows, max_cols)?;
        if row_start != row_end || col_start != col_end {
            return None;
        }

        Some((row_start, col_start))
    }

    #[allow(dead_code)]
    fn is_staged_cell_modified(
        session: &TableEditSession,
        row_idx: usize,
        col_idx: usize,
        current_row: &[String],
    ) -> bool {
        let (is_modified, _, _) = Self::cell_edit_state(session, row_idx, col_idx, current_row);
        is_modified
    }

    fn try_execute_sql(
        execute_sql_callback: &Arc<Mutex<Option<ResultGridSqlExecuteCallback>>>,
        sql: String,
    ) -> Result<(), String> {
        let callback = execute_sql_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(callback) = callback else {
            return Err("Edit callback is not connected.".to_string());
        };

        let cb = {
            let mut slot = callback
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.take()
        };
        let Some(mut cb) = cb else {
            return Err("Edit callback is not connected.".to_string());
        };

        let call_result = panic::catch_unwind(AssertUnwindSafe(|| cb(sql)));

        let mut slot = callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_none() {
            *slot = Some(cb);
        }
        drop(slot);

        match call_result {
            Ok(result) => result,
            Err(payload) => {
                Self::log_execute_callback_panic(payload.as_ref());
                Err("Internal error: result-grid execute callback panicked.".to_string())
            }
        }
    }

    fn panic_payload_to_string(payload: &(dyn Any + Send)) -> String {
        if let Some(msg) = payload.downcast_ref::<&str>() {
            (*msg).to_string()
        } else if let Some(msg) = payload.downcast_ref::<String>() {
            msg.clone()
        } else {
            "unknown panic payload".to_string()
        }
    }

    fn log_execute_callback_panic(payload: &(dyn Any + Send)) {
        let panic_payload = Self::panic_payload_to_string(payload);
        crate::utils::logging::log_error(
            "result_table::callback",
            &format!("result-grid execute callback panicked: {panic_payload}"),
        );
        eprintln!("result-grid execute callback panicked: {panic_payload}");
    }

    #[allow(dead_code)]
    fn rowid_for_row(
        row_index: usize,
        headers: &[String],
        full_data: &[Vec<String>],
    ) -> Result<(usize, String), String> {
        let rowid_col = Self::find_rowid_column_index(headers)
            .ok_or_else(|| "Editing requires a ROWID column in the result set.".to_string())?;
        let row = full_data
            .get(row_index)
            .ok_or_else(|| "Selected row is out of range.".to_string())?;
        let rowid = row
            .get(rowid_col)
            .ok_or_else(|| "ROWID value is missing for the selected row.".to_string())?
            .trim()
            .to_string();
        if rowid.is_empty() {
            return Err("Selected row has an empty ROWID value.".to_string());
        }
        Ok((rowid_col, rowid))
    }

    #[allow(dead_code)]
    fn push_unique_rowid(rowids: &mut Vec<String>, seen: &mut HashSet<String>, rowid_raw: &str) {
        let rowid = rowid_raw.trim();
        if rowid.is_empty() || seen.contains(rowid) {
            return;
        }
        let rowid_owned = rowid.to_string();
        seen.insert(rowid_owned.clone());
        rowids.push(rowid_owned);
    }

    #[allow(dead_code)]
    fn selected_rowids(
        table: &Table,
        headers: &[String],
        full_data: &[Vec<String>],
    ) -> Result<Vec<String>, String> {
        let (row_start, row_end) = Self::selected_row_range(table)
            .ok_or_else(|| "Select at least one row.".to_string())?;
        let rowid_col = Self::find_rowid_column_index(headers)
            .ok_or_else(|| "Editing requires a ROWID column in the result set.".to_string())?;

        Self::collect_rowids_in_range(row_start, row_end, rowid_col, full_data)
    }

    #[allow(dead_code)]
    fn collect_rowids_in_range(
        row_start: usize,
        row_end: usize,
        rowid_col: usize,
        full_data: &[Vec<String>],
    ) -> Result<Vec<String>, String> {
        let mut rowids = Vec::new();
        let mut seen = HashSet::new();
        for row_index in row_start..=row_end {
            let row = full_data
                .get(row_index)
                .ok_or_else(|| format!("Selected row {} is out of range.", row_index + 1))?;
            let rowid_raw = row.get(rowid_col).ok_or_else(|| {
                format!("ROWID value is missing for selected row {}.", row_index + 1)
            })?;
            if rowid_raw.trim().is_empty() {
                return Err(format!(
                    "Selected row {} has an empty ROWID value.",
                    row_index + 1
                ));
            }
            Self::push_unique_rowid(&mut rowids, &mut seen, rowid_raw);
        }

        if rowids.is_empty() {
            return Err("Selected rows do not contain valid ROWID values.".to_string());
        }
        Ok(rowids)
    }

    fn can_show_insert_row_action(source_sql: &str) -> bool {
        if source_sql.trim().is_empty() {
            return false;
        }
        if !QueryExecutor::is_rowid_edit_eligible_query(source_sql) {
            return false;
        }
        Self::resolve_target_table(source_sql).is_ok()
    }

    fn can_show_rowid_edit_actions(headers: &[String], source_sql: &str) -> bool {
        if !Self::can_show_insert_row_action(source_sql) {
            return false;
        }
        Self::find_rowid_column_index(headers).is_some()
    }

    fn prepare_edit_mode(
        headers: &[String],
        source_sql: &str,
    ) -> Result<EditModePreparation, String> {
        if !Self::can_show_rowid_edit_actions(headers, source_sql) {
            return Err("Current result set does not support ROWID-based editing.".to_string());
        }

        let table_name = Self::resolve_target_table(source_sql)?;
        let rowid_col = Self::find_rowid_column_index(headers)
            .ok_or_else(|| "Editing requires a ROWID column in the result set.".to_string())?;
        let editable_columns: Vec<(usize, String)> = headers
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != rowid_col)
            .filter_map(|(idx, name)| Self::editable_column_identifier(name).map(|id| (idx, id)))
            .collect();
        if editable_columns.is_empty() {
            return Err("No editable columns were detected in this result set.".to_string());
        }

        Ok(EditModePreparation {
            table_name,
            rowid_col,
            editable_columns,
        })
    }

    fn build_existing_edit_rows(
        full_data_snapshot: &[Vec<String>],
        rowid_col: usize,
    ) -> Result<(HashMap<String, Vec<String>>, Vec<String>, Vec<EditRowState>), String> {
        let mut original_rows_by_rowid = HashMap::new();
        let mut original_row_order = Vec::with_capacity(full_data_snapshot.len());
        let mut row_states = Vec::with_capacity(full_data_snapshot.len());

        for (row_idx, row) in full_data_snapshot.iter().enumerate() {
            let rowid = row
                .get(rowid_col)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    format!(
                        "Row {} cannot be edited because ROWID is missing or empty.",
                        row_idx + 1
                    )
                })?;
            if original_rows_by_rowid.contains_key(&rowid) {
                return Err(format!(
                    "Edit mode requires unique ROWID values (duplicate: {}).",
                    rowid
                ));
            }
            original_rows_by_rowid.insert(rowid.clone(), row.clone());
            original_row_order.push(rowid.clone());
            row_states.push(EditRowState::Existing {
                rowid,
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            });
        }

        Ok((original_rows_by_rowid, original_row_order, row_states))
    }

    pub fn is_save_pending(&self) -> bool {
        *self
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn is_edit_mode_enabled(&self) -> bool {
        self.edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    pub fn can_begin_edit_mode(&self) -> bool {
        if self.is_save_pending() {
            return false;
        }
        if self.is_streaming_in_progress() {
            return false;
        }

        if self.is_edit_mode_enabled() {
            return true;
        }

        let headers_snapshot = self
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let source_sql_text = self
            .source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        Self::prepare_edit_mode(&headers_snapshot, &source_sql_text).is_ok()
    }

    pub fn begin_edit_mode(&mut self) -> Result<String, String> {
        if self.is_save_pending() {
            return Err("Cannot begin edit mode while save is in progress.".to_string());
        }
        if self.is_streaming_in_progress() {
            return Err("Cannot begin edit mode while query rows are still loading.".to_string());
        }

        if self.is_edit_mode_enabled() {
            return Ok("Edit mode is already enabled.".to_string());
        }

        let headers_snapshot = self
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if headers_snapshot.is_empty() {
            return Err("No result columns available for editing.".to_string());
        }

        let source_sql_text = self
            .source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let EditModePreparation {
            table_name,
            rowid_col,
            editable_columns,
        } = Self::prepare_edit_mode(&headers_snapshot, &source_sql_text)?;

        let current_null_text = self.current_null_text();
        let full_data_snapshot = self
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let (original_rows_by_rowid, original_row_order, row_states) =
            Self::build_existing_edit_rows(&full_data_snapshot, rowid_col)?;

        *self
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        *self
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.pending_save_statement_signatures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();

        *self
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col,
            table_name,
            null_text: current_null_text,
            editable_columns,
            original_rows_by_rowid,
            original_row_order,
            deleted_rowids: Vec::new(),
            row_states,
        });

        self.refresh_auto_rowid_visibility();
        Ok("Edit mode enabled.".to_string())
    }

    pub fn insert_row_in_edit_mode(&mut self) -> Result<String, String> {
        if self.is_save_pending() {
            return Err("Cannot insert rows while save is in progress.".to_string());
        }
        if self.is_streaming_in_progress() {
            return Err("Cannot insert rows while query rows are still loading.".to_string());
        }

        // Commit any pending inline edit before inserting a new row so that
        // the previous cell's value is not silently lost.
        self.commit_active_inline_edit();

        let headers_len = self
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len();
        if headers_len == 0 {
            return Err("No result columns available for INSERT.".to_string());
        }

        let (rowid_col, first_edit_col) = {
            let guard = self
                .edit_session
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(session) = guard.as_ref() else {
                return Err("Enable edit mode first.".to_string());
            };
            (
                session.rowid_col,
                session.editable_columns.first().map(|(idx, _)| *idx),
            )
        };

        let new_row_index = {
            let mut full_data = self
                .full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut guard = self
                .edit_session
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(session) = guard.as_mut() else {
                return Err("Edit mode is no longer active.".to_string());
            };
            let new_row_index = full_data.len();
            let mut row = vec![String::new(); headers_len];
            if rowid_col < row.len() {
                row[rowid_col].clear();
            }
            full_data.push(row);
            session.row_states.push(EditRowState::Inserted {
                explicit_null_cols: HashSet::new(),
            });
            new_row_index
        };

        self.set_table_rows_for_current_font((new_row_index + 1) as i32);
        self.table.set_row_position(new_row_index as i32);
        self.sync_table_viewport_state();

        if let Some(first_col) = first_edit_col {
            self.table.set_selection(
                new_row_index as i32,
                first_col as i32,
                new_row_index as i32,
                first_col as i32,
            );
            let profile = self.font_settings.profile();
            let size = self.font_settings.size();
            if let Some(value) = Self::show_inline_cell_editor(
                &self.table,
                new_row_index as i32,
                first_col as i32,
                "",
                profile,
                size,
                &self.active_inline_edit,
            ) {
                let mut full_data = self
                    .full_data
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(row) = full_data.get_mut(new_row_index) {
                    if first_col >= row.len() {
                        row.resize(first_col + 1, String::new());
                    }
                    row[first_col] = value.clone();
                }
                let mut guard = self
                    .edit_session
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(session) = guard.as_mut() {
                    let is_explicit_null = session
                        .row_states
                        .get(new_row_index)
                        .map(|row_state| {
                            Self::input_maps_to_explicit_null(row_state, &value, &session.null_text)
                        })
                        .unwrap_or(false);
                    let _ = Self::set_row_cell_explicit_null(
                        session,
                        new_row_index,
                        first_col,
                        is_explicit_null,
                    );
                }
            } else {
                {
                    let mut full_data = self
                        .full_data
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let mut guard = self
                        .edit_session
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if new_row_index < full_data.len() {
                        full_data.remove(new_row_index);
                    }
                    if let Some(session) = guard.as_mut() {
                        if new_row_index < session.row_states.len() {
                            session.row_states.remove(new_row_index);
                        }
                    }
                }

                let new_len = self
                    .full_data
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .len();
                self.set_table_rows_for_current_font(new_len as i32);
                if new_len > 0 {
                    let row = (new_row_index).min(new_len.saturating_sub(1)) as i32;
                    let col = self.table.get_selection().1.max(0);
                    self.table.set_selection(row, col, row, col);
                }
                self.table.redraw();
                return Ok("Cancelled row insertion and removed staged row.".to_string());
            }
        }

        // Redraw is already performed by sync_table_viewport_state() above.
        Ok("Inserted a new staged row.".to_string())
    }

    pub fn delete_selected_rows_in_edit_mode(&mut self) -> Result<String, String> {
        if self.is_save_pending() {
            return Err("Cannot delete rows while save is in progress.".to_string());
        }
        if self.is_streaming_in_progress() {
            return Err("Cannot delete rows while query rows are still loading.".to_string());
        }

        // Commit any pending inline edit before modifying the row set so that
        // the edited value lands on the correct row and the editor widget is
        // cleaned up (prevents stale-index writes after rows are removed).
        self.commit_active_inline_edit();

        let (row_start, row_end) = Self::selected_row_range(&self.table)
            .ok_or_else(|| "Select row(s) to delete.".to_string())?;

        let mut removed = 0usize;
        {
            let mut full_data = self
                .full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut guard = self
                .edit_session
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(session) = guard.as_mut() else {
                return Err("Enable edit mode first.".to_string());
            };

            if full_data.len() != session.row_states.len() {
                return Err("Edit session and table rows are out of sync.".to_string());
            }
            if full_data.is_empty() {
                return Err("No staged rows are available to delete.".to_string());
            }

            let mut deleted_set: HashSet<String> = session.deleted_rowids.iter().cloned().collect();
            if row_start >= full_data.len() {
                return Err("Selected rows are outside the staged row range.".to_string());
            }
            let end = row_end.min(full_data.len().saturating_sub(1));
            for idx in (row_start..=end).rev() {
                if idx >= full_data.len() || idx >= session.row_states.len() {
                    continue;
                }
                if let EditRowState::Existing { rowid, .. } = &session.row_states[idx] {
                    if deleted_set.insert(rowid.clone()) {
                        session.deleted_rowids.push(rowid.clone());
                    }
                }
                full_data.remove(idx);
                session.row_states.remove(idx);
                removed += 1;
            }
        }

        let new_len = self
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len();

        if removed == 0 {
            return Err("No selected rows were available to delete.".to_string());
        }

        self.set_table_rows_for_current_font(new_len as i32);
        if new_len > 0 {
            let row = row_start.min(new_len.saturating_sub(1)) as i32;
            let col = self.table.get_selection().1.max(0);
            self.table.set_selection(row, col, row, col);
        }
        self.table.redraw();
        Ok(format!("Staged delete for {} row(s).", removed))
    }

    pub fn save_edit_mode(&mut self) -> Result<String, String> {
        if self.is_save_pending() {
            return Err("Save is already in progress.".to_string());
        }
        if self.is_streaming_in_progress() {
            return Err("Cannot save edits while query rows are still loading.".to_string());
        }

        // If the user clicks Save while an inline editor is still focused,
        // force focus back to the table first so FLTK commits any pending
        // in-widget edit state before we snapshot staged rows.
        if !self.table.was_deleted() {
            let _ = self.table.take_focus();
            if app::is_ui_thread() {
                app::flush();
            }
        }
        self.commit_active_inline_edit();

        let session = self
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned()
            .ok_or_else(|| "Enable edit mode first.".to_string())?;
        let rows = self
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();

        if rows.len() != session.row_states.len() {
            return Err("Edit session and table rows are out of sync.".to_string());
        }

        let mut statements = Vec::new();

        if !session.deleted_rowids.is_empty() {
            // Oracle limits IN-list to 1000 elements (ORA-01795).  Chunk
            // deleted ROWIDs so each DELETE stays within the limit.
            let table_ref = Self::quote_qualified_identifier(&session.table_name);
            for chunk in session.deleted_rowids.chunks(1000) {
                let delete_where = if chunk.len() == 1 {
                    format!("ROWID = {}", Self::sql_string_literal(&chunk[0]))
                } else {
                    let rowid_literals = chunk
                        .iter()
                        .map(|rowid| Self::sql_string_literal(rowid))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("ROWID IN ({rowid_literals})")
                };
                statements.push(format!("DELETE FROM {} WHERE {}", table_ref, delete_where));
            }
        }

        for (idx, row_state) in session.row_states.iter().enumerate() {
            let Some(row) = rows.get(idx) else {
                continue;
            };
            match row_state {
                EditRowState::Existing { rowid, .. } => {
                    let Some(original_row) = session.original_rows_by_rowid.get(rowid) else {
                        continue;
                    };
                    let mut assignments = Vec::new();
                    for (col_idx, column_id) in session.editable_columns.iter() {
                        let new_value = row.get(*col_idx).cloned().unwrap_or_default();
                        let old_value = original_row.get(*col_idx).cloned().unwrap_or_default();
                        let is_explicit_null =
                            Self::row_cell_is_explicit_null(&session, idx, *col_idx);
                        // Skip redundant SET col = NULL when the original
                        // value was already NULL (avoids unnecessary DB I/O).
                        if is_explicit_null
                            && Self::value_represents_null(&old_value, &session.null_text)
                        {
                            continue;
                        }
                        if is_explicit_null || new_value != old_value {
                            assignments.push(format!(
                                "{} = {}",
                                column_id,
                                if is_explicit_null {
                                    "NULL".to_string()
                                } else {
                                    Self::sql_literal_from_input_with_null_text(
                                        &new_value,
                                        &session.null_text,
                                    )?
                                }
                            ));
                        }
                    }
                    if !assignments.is_empty() {
                        statements.push(format!(
                            "UPDATE {} SET {} WHERE ROWID = {}",
                            Self::quote_qualified_identifier(&session.table_name),
                            assignments.join(", "),
                            Self::sql_string_literal(rowid)
                        ));
                    }
                }
                EditRowState::Inserted { .. } => {
                    let mut column_names = Vec::new();
                    let mut values = Vec::new();
                    for (col_idx, column_id) in session.editable_columns.iter() {
                        let value = row.get(*col_idx).cloned().unwrap_or_default();
                        let is_explicit_null =
                            Self::row_cell_is_explicit_null(&session, idx, *col_idx);
                        // 값이 비어 있는 컬럼은 INSERT 목록에서 제외해
                        // DB가 DEFAULT 값을 적용할 수 있게 한다.
                        // 명시적으로 NULL을 원할 때는 '=NULL' 또는 단독 NULL 입력 사용.
                        if value.is_empty() && !is_explicit_null {
                            continue;
                        }
                        let literal = if is_explicit_null {
                            "NULL".to_string()
                        } else {
                            Self::sql_literal_from_input_with_null_text(&value, &session.null_text)?
                        };
                        column_names.push(column_id.clone());
                        values.push(literal);
                    }
                    if !column_names.is_empty() {
                        statements.push(format!(
                            "INSERT INTO {} ({}) VALUES ({})",
                            Self::quote_qualified_identifier(&session.table_name),
                            column_names.join(", "),
                            values.join(", ")
                        ));
                    }
                }
            }
        }

        if statements.is_empty() {
            // Keep edit mode active when there is nothing to persist yet.
            // This prevents accidental edit-session termination (for example,
            // after opening edit mode and pressing Save before making any
            // changes), which would otherwise leave the user in a confusing
            // partially edited state without applying anything.
            return Ok("No staged changes to save. Edit mode is still enabled.".to_string());
        }

        // Wrap multiple DML statements in an anonymous PL/SQL block so that
        // they are treated as a single unit by the executor.  When auto-commit
        // is enabled this prevents partial commits: the whole block either
        // succeeds (and the client commits once) or fails (nothing is committed).
        let dml_script = if statements.len() > 1 {
            format!("BEGIN\n{};\nEND;", statements.join(";\n"))
        } else {
            let mut s = statements.join("");
            s.push(';');
            s
        };
        let request_id = mutex_fetch_add_u64(&self.next_save_request_id, 1);
        let request_tag = format!("SQ_SAVE_REQUEST:{request_id}");
        let tagged_script = format!(
            "/* {request_tag} */
{dml_script}"
        );
        // Execute only staged DML during save. Re-running the source SELECT as
        // part of the same request can execute unintended extra statements
        // (when the original text was a multi-statement script) and can also
        // report the save as failed even after DML already succeeded.
        let script = tagged_script;
        // Keep SQL-signature matching resilient when downstream execution paths
        // strip leading comments from the statement text. The request tag comment
        // is still used separately via `pending_save_request_tag`.
        let save_signature = Self::canonical_sql_signature(&dml_script);
        let save_statement_signatures: Vec<String> = statements
            .iter()
            .map(|statement| Self::canonical_sql_signature(statement))
            .filter(|signature| !signature.is_empty())
            .collect();
        *self
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        *self
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(save_signature);
        *self
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(request_tag);
        *self
            .pending_save_statement_signatures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = save_statement_signatures;

        if let Err(err) = Self::try_execute_sql(&self.execute_sql_callback, script) {
            *self
                .pending_save_request
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
            *self
                .pending_save_sql_signature
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            *self
                .pending_save_request_tag
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            self.pending_save_statement_signatures
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear();
            return Err(err);
        }

        Ok(format!(
            "Saving {} staged statement(s)...",
            statements.len()
        ))
    }

    pub fn cancel_edit_mode(&mut self) -> Result<String, String> {
        if *self
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            return Err("Cannot cancel edit mode while save is in progress.".to_string());
        }
        if self.is_streaming_in_progress() {
            return Err("Cannot cancel edit mode while query rows are still loading.".to_string());
        }

        // Discard any pending inline edit without committing — the user is
        // cancelling all staged changes so the editor value must not be
        // written back into the data that is about to be restored.
        Self::clear_active_inline_edit_widget(&self.active_inline_edit);

        let session = self
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .ok_or_else(|| "Edit mode is not active.".to_string())?;

        *self
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        *self
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.pending_save_statement_signatures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.clear_pending_stream_buffers();

        // Cancelling edit mode is an explicit user intent to discard staged
        // state. Drop any saved pre-query backup as well so a later unrelated
        // query failure cannot resurrect cancelled edits.
        self.set_query_edit_backup(None);

        let mut restored_rows = Vec::with_capacity(session.original_row_order.len());
        for rowid in session.original_row_order.iter() {
            if let Some(row) = session.original_rows_by_rowid.get(rowid) {
                restored_rows.push(row.clone());
            }
        }
        *self
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = restored_rows;
        let new_len = self
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len();
        self.set_table_rows_for_current_font(new_len as i32);
        self.recalculate_widths_for_current_font();
        self.refresh_auto_rowid_visibility();
        self.table.redraw();
        Ok("Cancelled staged edits and restored original rows.".to_string())
    }

    #[allow(dead_code)]
    fn show_update_cell_dialog(
        table: &Table,
        headers: &Arc<Mutex<Vec<String>>>,
        full_data: &Arc<Mutex<Vec<Vec<String>>>>,
        source_sql: &Arc<Mutex<String>>,
        execute_sql_callback: &Arc<Mutex<Option<ResultGridSqlExecuteCallback>>>,
        null_text: &Arc<Mutex<String>>,
        context_cell: Option<(usize, usize)>,
    ) {
        let Some((row_index, col_index)) = Self::resolve_update_target_cell(
            table.get_selection(),
            table.rows().max(0) as usize,
            table.cols().max(0) as usize,
            context_cell,
        ) else {
            crate::ui::alert_on_main("Select a single cell to update.");
            return;
        };

        let headers_snapshot = headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let source_sql_text = source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();

        let (rowid_col, rowid_value, current_value) = {
            let data_guard = full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (rowid_col, rowid_value) =
                match Self::rowid_for_row(row_index, &headers_snapshot, &data_guard) {
                    Ok(v) => v,
                    Err(err) => {
                        crate::ui::alert_on_main(&err);
                        return;
                    }
                };
            let current_value = data_guard
                .get(row_index)
                .and_then(|row| row.get(col_index))
                .cloned()
                .unwrap_or_default();
            (rowid_col, rowid_value, current_value)
        };
        if col_index == rowid_col {
            crate::ui::alert_on_main("ROWID cell cannot be updated.");
            return;
        }

        let Some(column_name) = headers_snapshot.get(col_index).cloned() else {
            crate::ui::alert_on_main("Selected column is out of range.");
            return;
        };
        let Some(column_identifier) = Self::editable_column_identifier(&column_name) else {
            crate::ui::alert_on_main(
                "Selected column cannot be mapped to an editable column name.",
            );
            return;
        };

        let prompt = format!(
            "New value for {} (blank/NULL -> NULL, '=expr' -> SQL expression)",
            column_name
        );
        let Some(input) = crate::ui::input_on_main(&prompt, &current_value) else {
            return;
        };

        let table_name = match Self::resolve_target_table(&source_sql_text) {
            Ok(name) => name,
            Err(err) => {
                crate::ui::alert_on_main(&err);
                return;
            }
        };

        let current_null_text = null_text
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let sql = format!(
            "UPDATE {} SET {} = {} WHERE ROWID = {}",
            Self::quote_qualified_identifier(&table_name),
            column_identifier,
            match Self::sql_literal_from_input_with_null_text(&input, &current_null_text) {
                Ok(value) => value,
                Err(err) => {
                    crate::ui::alert_on_main(&err);
                    return;
                }
            },
            Self::sql_string_literal(&rowid_value)
        );
        let script = Self::compose_edit_script(&sql, &source_sql_text);
        if let Err(err) = Self::try_execute_sql(execute_sql_callback, script) {
            crate::ui::alert_on_main(&err);
        }
    }

    #[allow(dead_code)]
    fn show_delete_row_dialog(
        table: &Table,
        headers: &Arc<Mutex<Vec<String>>>,
        full_data: &Arc<Mutex<Vec<Vec<String>>>>,
        source_sql: &Arc<Mutex<String>>,
        execute_sql_callback: &Arc<Mutex<Option<ResultGridSqlExecuteCallback>>>,
    ) {
        let headers_snapshot = headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let source_sql_text = source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();

        let rowids = {
            let data_guard = full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match Self::selected_rowids(table, &headers_snapshot, &data_guard) {
                Ok(v) => v,
                Err(err) => {
                    crate::ui::alert_on_main(&err);
                    return;
                }
            }
        };

        let delete_count = rowids.len();
        let confirm = crate::ui::choice2_on_main(
            &format!("Delete {} selected row(s)?", delete_count),
            "Cancel",
            "Delete",
            "",
        );
        if confirm != Some(1) {
            return;
        }

        let table_name = match Self::resolve_target_table(&source_sql_text) {
            Ok(name) => name,
            Err(err) => {
                crate::ui::alert_on_main(&err);
                return;
            }
        };

        let where_clause = if rowids.len() == 1 {
            format!("ROWID = {}", Self::sql_string_literal(&rowids[0]))
        } else {
            let literals = rowids
                .iter()
                .map(|rowid| Self::sql_string_literal(rowid))
                .collect::<Vec<_>>()
                .join(", ");
            format!("ROWID IN ({literals})")
        };
        let sql = format!(
            "DELETE FROM {} WHERE {}",
            Self::quote_qualified_identifier(&table_name),
            where_clause
        );
        let script = Self::compose_edit_script(&sql, &source_sql_text);
        if let Err(err) = Self::try_execute_sql(execute_sql_callback, script) {
            crate::ui::alert_on_main(&err);
        }
    }

    #[allow(dead_code)]
    fn show_insert_row_dialog(
        table: &Table,
        headers: &Arc<Mutex<Vec<String>>>,
        full_data: &Arc<Mutex<Vec<Vec<String>>>>,
        source_sql: &Arc<Mutex<String>>,
        execute_sql_callback: &Arc<Mutex<Option<ResultGridSqlExecuteCallback>>>,
        null_text: &Arc<Mutex<String>>,
    ) {
        let headers_snapshot = headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let source_sql_text = source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();

        let table_name = match Self::resolve_target_table(&source_sql_text) {
            Ok(name) => name,
            Err(err) => {
                crate::ui::alert_on_main(&err);
                return;
            }
        };

        let rowid_col = Self::find_rowid_column_index(&headers_snapshot);
        let editable_columns: Vec<(usize, String)> = headers_snapshot
            .iter()
            .enumerate()
            .filter(|(idx, _)| Some(*idx) != rowid_col)
            .filter_map(|(idx, name)| {
                Self::editable_column_identifier(name)
                    .map(|column_identifier| (idx, column_identifier))
            })
            .collect();
        if editable_columns.is_empty() {
            crate::ui::alert_on_main("No editable columns are available for INSERT.");
            return;
        }

        let selected_row = Self::selected_row(table);
        let selected_row_values = selected_row.and_then(|row_index| {
            full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(row_index)
                .cloned()
        });
        let current_null_text = null_text
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let mut value_literals: Vec<String> = Vec::with_capacity(editable_columns.len());
        let mut column_names: Vec<String> = Vec::with_capacity(editable_columns.len());
        for (col_idx, column_identifier) in editable_columns {
            let Some(column_name) = headers_snapshot.get(col_idx).cloned() else {
                continue;
            };
            let default_value = selected_row_values
                .as_ref()
                .and_then(|row| row.get(col_idx))
                .cloned()
                .unwrap_or_default();
            let prompt = format!(
                "Value for {} (blank/NULL -> NULL, '=expr' -> SQL expression)",
                column_name
            );
            let Some(input) = crate::ui::input_on_main(&prompt, &default_value) else {
                return;
            };
            column_names.push(column_identifier);
            let literal =
                match Self::sql_literal_from_input_with_null_text(&input, &current_null_text) {
                    Ok(value) => value,
                    Err(err) => {
                        crate::ui::alert_on_main(&err);
                        return;
                    }
                };
            value_literals.push(literal);
        }

        if column_names.is_empty() || value_literals.is_empty() {
            crate::ui::alert_on_main("No values were provided for INSERT.");
            return;
        }

        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            Self::quote_qualified_identifier(&table_name),
            column_names.join(", "),
            value_literals.join(", ")
        );
        let script = Self::compose_edit_script(&sql, &source_sql_text);
        if let Err(err) = Self::try_execute_sql(execute_sql_callback, script) {
            crate::ui::alert_on_main(&err);
        }
    }

    fn show_context_menu(
        table: &Table,
        headers: &Arc<Mutex<Vec<String>>>,
        full_data: &Arc<Mutex<Vec<Vec<String>>>>,
        hidden_auto_rowid_col: &Arc<Mutex<Option<usize>>>,
        _source_sql: &Arc<Mutex<String>>,
        _execute_sql_callback: &Arc<Mutex<Option<ResultGridSqlExecuteCallback>>>,
        edit_session: &Arc<Mutex<Option<TableEditSession>>>,
        pending_save_request: &Arc<Mutex<bool>>,
        active_inline_edit: &Arc<Mutex<Option<ActiveInlineEdit>>>,
        lazy_fetch_session: &Arc<Mutex<Option<u64>>>,
        lazy_fetch_callback: &LazyFetchCallback,
        pending_lazy_actions: &Arc<Mutex<VecDeque<(u64, LazyFetchPendingAction)>>>,
        context_action_callback: &ResultTableContextActionCallback,
    ) {
        let mouse_x = app::event_x();
        let mouse_y = app::event_y();

        let mut table = table.clone();
        let hidden_col_for_hit = *hidden_auto_rowid_col
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let clicked_cell = Self::get_cell_at_mouse(&table);
        let clicked_row_header = if clicked_cell.is_none() {
            Self::get_row_header_at_mouse(&table)
        } else {
            None
        };
        let clicked_col_header = if clicked_cell.is_none() && clicked_row_header.is_none() {
            Self::get_col_header_at_mouse(&table)
        } else {
            None
        };
        let clicked_empty_body_with_columns = clicked_cell.is_none()
            && clicked_row_header.is_none()
            && clicked_col_header.is_none()
            && table.rows() <= 0
            && Self::has_visible_table_columns(&table, hidden_col_for_hit)
            && Self::is_mouse_inside_table_body(&table, mouse_x, mouse_y);

        if !Self::should_show_context_menu_for_hit(
            clicked_cell,
            clicked_row_header,
            clicked_col_header,
            clicked_empty_body_with_columns,
        ) {
            return;
        }

        // Give focus and potentially select cell under mouse for better UX
        let _ = table.take_focus();
        if let Some((row, col)) = clicked_cell {
            // If the cell under mouse is not already in the selection, select it.
            if !Self::selection_contains_cell(table.get_selection(), row, col) {
                table.set_selection(row, col, row, col);
                table.redraw();
            }
        } else if let Some(row) = clicked_row_header {
            let cols = table.cols();
            if cols <= 0 {
                return;
            }
            // Row-header context actions (delete/insert defaults) should target the clicked row.
            table.set_selection(row, 0, row, cols.saturating_sub(1));
            table.redraw();
        } else if let Some(col) = clicked_col_header {
            let rows = table.rows();
            if rows > 0 {
                table.set_selection(0, col, rows.saturating_sub(1), col);
                table.redraw();
            }
        }

        // Prevent menu from being added to parent container
        let current_group = fltk::group::Group::try_current();
        fltk::group::Group::set_current(None::<&fltk::group::Group>);

        let mut menu = MenuButton::new(mouse_x, mouse_y, 0, 0, None);
        menu.set_color(theme::panel_raised());
        menu.set_text_color(theme::text_primary());
        let can_set_null = if *pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            false
        } else {
            let session_guard = edit_session
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(session) = session_guard.as_ref() {
                if let Some((row_start, col_start, row_end, col_end)) =
                    Self::normalized_selection_bounds_with_limits(
                        table.get_selection(),
                        table.rows().max(0) as usize,
                        table.cols().max(0) as usize,
                    )
                {
                    let editable_cols: HashSet<usize> = session
                        .editable_columns
                        .iter()
                        .map(|(idx, _)| *idx)
                        .collect();
                    let mut has_target = false;
                    for row_idx in row_start..=row_end {
                        if row_idx >= session.row_states.len() {
                            continue;
                        }
                        for col_idx in col_start..=col_end {
                            if col_idx == session.rowid_col || !editable_cols.contains(&col_idx) {
                                continue;
                            }
                            has_target = true;
                            break;
                        }
                        if has_target {
                            break;
                        }
                    }
                    has_target
                } else {
                    false
                }
            } else {
                false
            }
        };

        let mut menu_items = vec![
            "Close",
            "Close All",
            "Save as CSV (Ctrl+E)",
            "Copy",
            "Copy with Headers",
            "Copy All",
        ];
        if can_set_null {
            menu_items.push("Set Null");
        }
        menu.add_choice(&menu_items.join("|"));

        if let Some(ref group) = current_group {
            fltk::group::Group::set_current(Some(group));
        }

        if let Some(choice) = menu.popup() {
            let hidden_col = hidden_col_for_hit;
            let choice_label = choice.label().unwrap_or_default();
            match choice_label.as_str() {
                "Copy" => {
                    Self::copy_selected_to_clipboard(&table, full_data, hidden_col);
                }
                "Copy with Headers" => {
                    Self::copy_selected_with_headers(&table, headers, full_data, hidden_col);
                }
                "Copy All" => {
                    if Self::queue_lazy_action_if_active(
                        lazy_fetch_session,
                        lazy_fetch_callback,
                        pending_lazy_actions,
                        LazyFetchPendingAction::CopyAll,
                    ) {
                        return;
                    }
                    Self::copy_all_to_clipboard(headers, full_data, hidden_col);
                }
                "Save as CSV (Ctrl+E)" => {
                    Self::schedule_context_action_callback(
                        context_action_callback,
                        ResultTableContextAction::ExportCsv,
                    );
                }
                "Close" => {
                    Self::schedule_context_action_callback(
                        context_action_callback,
                        ResultTableContextAction::Close,
                    );
                }
                "Close All" => {
                    Self::schedule_context_action_callback(
                        context_action_callback,
                        ResultTableContextAction::CloseAll,
                    );
                }
                "Set Null" => {
                    if let Err(err) = Self::set_selected_cells_to_null_in_edit_mode(
                        &table,
                        full_data,
                        edit_session,
                        pending_save_request,
                        active_inline_edit,
                    ) {
                        if !err.is_empty() {
                            crate::ui::alert_on_main(&err);
                        }
                    }
                }
                _ => {}
            }
        }

        MenuButton::delete(menu);
    }

    fn copy_selected_to_clipboard(
        table: &Table,
        full_data: &Arc<Mutex<Vec<Vec<String>>>>,
        hidden_col: Option<usize>,
    ) -> usize {
        let Some((row_top, col_left, row_bot, col_right)) =
            Self::normalized_selection_bounds_with_limits(
                table.get_selection(),
                table.rows().max(0) as usize,
                table.cols().max(0) as usize,
            )
        else {
            return 0;
        };

        let rows = row_bot - row_top + 1;
        let visible_cols = Self::visible_column_indices_in_range(col_left, col_right, hidden_col);
        if visible_cols.is_empty() {
            return 0;
        }
        let cell_count = rows * visible_cols.len();

        let full_data = full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut result = String::with_capacity(rows * visible_cols.len() * 16);
        for row in row_top..=row_bot {
            if row > row_top {
                result.push('\n');
            }
            for (visible_idx, col) in visible_cols.iter().enumerate() {
                if visible_idx > 0 {
                    result.push('\t');
                }
                if let Some(val) = full_data.get(row).and_then(|r| r.get(*col)) {
                    result.push_str(&Self::escape_cell_for_clipboard(val));
                }
            }
        }

        if !result.is_empty() {
            app::copy(&result);
            cell_count
        } else {
            0
        }
    }

    fn copy_selected_with_headers(
        table: &Table,
        headers: &Arc<Mutex<Vec<String>>>,
        full_data: &Arc<Mutex<Vec<Vec<String>>>>,
        hidden_col: Option<usize>,
    ) -> usize {
        let Some((row_top, col_left, row_bot, col_right)) =
            Self::normalized_selection_bounds_with_limits(
                table.get_selection(),
                table.rows().max(0) as usize,
                table.cols().max(0) as usize,
            )
        else {
            return 0;
        };

        let rows = row_bot - row_top + 1;
        let visible_cols = Self::visible_column_indices_in_range(col_left, col_right, hidden_col);
        if visible_cols.is_empty() {
            return 0;
        }
        let cell_count = rows * visible_cols.len();
        let mut result = String::with_capacity((rows + 1) * visible_cols.len() * 16);

        {
            let headers = headers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for (visible_idx, col) in visible_cols.iter().enumerate() {
                if visible_idx > 0 {
                    result.push('\t');
                }
                if let Some(h) = headers.get(*col) {
                    result.push_str(&Self::escape_cell_for_clipboard(h));
                }
            }
        }
        result.push('\n');

        {
            let full_data = full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for row in row_top..=row_bot {
                if row > row_top {
                    result.push('\n');
                }
                for (visible_idx, col) in visible_cols.iter().enumerate() {
                    if visible_idx > 0 {
                        result.push('\t');
                    }
                    if let Some(val) = full_data.get(row).and_then(|r| r.get(*col)) {
                        result.push_str(&Self::escape_cell_for_clipboard(val));
                    }
                }
            }
        }

        if !result.is_empty() {
            app::copy(&result);
            cell_count
        } else {
            0
        }
    }

    fn copy_all_to_clipboard(
        headers: &Arc<Mutex<Vec<String>>>,
        full_data: &Arc<Mutex<Vec<Vec<String>>>>,
        hidden_col: Option<usize>,
    ) {
        let header_values = {
            let headers = headers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::visible_headers(&headers, hidden_col)
        };
        let header_line = header_values
            .iter()
            .map(|h| Self::escape_cell_for_clipboard(h))
            .collect::<Vec<_>>()
            .join("\t");

        let row_count = full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len();
        let mut result = String::with_capacity(row_count * 16 + header_line.len() + 1);

        result.push_str(&header_line);
        result.push('\n');

        let full_data = full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for row in full_data.iter() {
            let visible_row = Self::visible_row_values_internal(row, hidden_col);
            for (i, cell) in visible_row.iter().enumerate() {
                if i > 0 {
                    result.push('\t');
                }
                result.push_str(&Self::escape_cell_for_clipboard(cell));
            }
            result.push('\n');
        }

        if !result.is_empty() {
            app::copy(&result);
        }
    }

    pub fn display_result(&mut self, result: &QueryResult) {
        mutex_store_bool(&self.streaming_in_progress, false);
        *self
            .lazy_fetch_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.lazy_fetch_more_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();

        // Query completion can race with an open inline editor focus change.
        // Commit any pending in-cell value first so failed/cancelled queries
        // do not silently discard the user's last typed value.
        self.commit_active_inline_edit();

        let (save_requested, save_still_pending) = {
            let mut pending_guard = self
                .pending_save_request
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut save_signature = self
                .pending_save_sql_signature
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut save_tag = self
                .pending_save_request_tag
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut save_statement_signatures = self
                .pending_save_statement_signatures
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());

            if !*pending_guard {
                *save_signature = None;
                *save_tag = None;
                save_statement_signatures.clear();
                (false, false)
            } else {
                let matches_save = Self::is_pending_save_terminal_result(
                    save_tag.as_deref(),
                    save_signature.as_deref(),
                    &save_statement_signatures,
                    result,
                );

                if matches_save {
                    *pending_guard = false;
                    *save_signature = None;
                    *save_tag = None;
                    save_statement_signatures.clear();
                    (true, false)
                } else {
                    // While a save is pending, ignore out-of-order statement
                    // results (both success and failure) that do not match the
                    // in-flight save request. Clearing the pending flag here on
                    // an unrelated failure can unlock edit actions even though
                    // the save is still executing.
                    (false, true)
                }
            }
        };
        if save_still_pending {
            // Ignore out-of-order results while a save response is still pending.
            return;
        }
        let is_edit_mode_enabled = self
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some();

        if save_requested {
            if result.success {
                self.clear_pending_stream_buffers();
                self.set_query_edit_backup(None);
                *self
                    .edit_session
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
                self.refresh_auto_rowid_visibility();
                // Save requests are internal executions for the current editable
                // result set. Keep the staged grid rows visible after success
                // instead of replacing them with an unrelated statement payload.
                // In rare out-of-order/cancellation paths Oracle can still
                // surface a SELECT-shaped terminal packet; treat it the same as
                // DML success and preserve the existing grid.
                self.table.redraw();
                return;
            } else {
                self.clear_pending_stream_buffers();
                // Save failed: keep staged edits so users can fix and retry.
                // Even if edit_session was unexpectedly cleared, do not replace
                // the current grid with a transient error row.
                if !is_edit_mode_enabled {
                    let _ = self.restore_query_edit_backup();
                }
                return;
            }
        } else if !result.success {
            if is_edit_mode_enabled {
                self.clear_pending_stream_buffers();
                // A regular query failed/cancelled while edit mode is active.
                // Keep the staged grid data intact so the user can continue editing
                // or retry explicitly instead of losing in-progress changes.
                self.set_query_edit_backup(None);
                return;
            }
            if self.restore_query_edit_backup() {
                self.clear_pending_stream_buffers();
                return;
            }
            if result.is_select
                && Self::is_query_cancel_message(&result.message)
                && self.table.rows() > 0
            {
                self.clear_pending_stream_buffers();
                *self
                    .source_sql
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = result.sql.clone();
                self.refresh_auto_rowid_visibility();
                self.table.redraw();
                return;
            }
        } else if is_edit_mode_enabled && !result.is_select {
            self.clear_pending_stream_buffers();
            // Non-save non-select statements (e.g. COMMIT/ROLLBACK/DDL) can be
            // executed while a grid edit session is active. Keep staged rows
            // and edit mode intact so ad-hoc statement success does not
            // silently discard unsaved result-grid edits.
            self.set_query_edit_backup(None);
            self.table.redraw();
            return;
        } else {
            self.set_query_edit_backup(None);
            *self
                .edit_session
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        }
        self.clear_sort_state();
        *self
            .source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = if result.is_select {
            result.sql.clone()
        } else {
            String::new()
        };
        let should_render_message_only = !result.is_select
            || (!result.success && result.rows.is_empty() && result.columns.is_empty());

        if should_render_message_only {
            self.clear_pending_stream_buffers();
            let font_size = self.font_settings.size();
            let max_cell_display_chars = *self
                .max_cell_display_chars
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            self.set_table_rows_for_current_font(1);
            self.table.set_cols(1);
            let message_width =
                Self::estimate_display_width(&result.message, font_size, max_cell_display_chars)
                    .clamp(200, 1200);
            self.table.set_col_width(0, message_width);
            *self
                .headers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = vec!["Result".to_string()];
            *self
                .full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                vec![vec![result.message.clone()]];
            *self
                .hidden_auto_rowid_col
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            self.table.redraw();
            return;
        }

        if result.rows.is_empty() && result.row_count > 0 && self.table.rows() > 0 {
            let col_names: Vec<String> = result.columns.iter().map(|c| c.name.clone()).collect();
            let col_count = col_names.len() as i32;
            if self.table.cols() < col_count {
                self.table.set_cols(col_count);
            }
            *self
                .headers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = col_names;
            self.refresh_auto_rowid_visibility();
            self.table.redraw();
            return;
        }

        self.clear_pending_stream_buffers();
        let col_names: Vec<String> = result.columns.iter().map(|c| c.name.clone()).collect();
        let row_count = result.rows.len() as i32;
        let col_count = col_names.len() as i32;

        // Update table dimensions — no internal CellMatrix to rebuild
        self.set_table_rows_for_current_font(row_count);
        self.table.set_cols(col_count);

        let font_size = self.font_settings.size();
        let max_cell_display_chars = *self
            .max_cell_display_chars
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let widths = Self::compute_column_widths(
            &col_names,
            &result.rows,
            font_size,
            max_cell_display_chars,
        );
        self.apply_widths_to_table(&widths);
        *self
            .pending_widths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = widths;

        // Store data directly — draw_cell reads from full_data on demand.
        // No per-cell set_cell_value calls needed!
        *self
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = result.rows.clone();
        *self
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = col_names;
        self.refresh_auto_rowid_visibility();
        self.table.redraw();
    }

    pub fn start_streaming(&mut self, headers: &[String]) {
        mutex_store_bool(&self.streaming_in_progress, true);
        *self
            .lazy_fetch_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.lazy_fetch_more_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();

        let save_pending = *self
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let had_edit_session = self
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some();
        if had_edit_session || save_pending {
            // Query-start events can arrive while an inline editor still has focus.
            // Persist the typed value first so cancel/failure paths do not drop it.
            self.commit_active_inline_edit();
        }

        // Capture the edit session snapshot only after inline edits are committed.
        // This keeps row-level metadata (e.g. explicit NULL markers) consistent
        // with the staged full_data backup restored after query failure/cancel.
        let edit_session_snapshot = self
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();

        if save_pending {
            if !had_edit_session {
                Self::clear_active_inline_edit_widget(&self.active_inline_edit);
            }
            // Ignore out-of-order SELECT start packets while a save request is
            // still pending. Since this path does not actually enter streaming,
            // keep the flag cleared so edit controls are not blocked waiting for
            // a finish event that may never arrive.
            mutex_store_bool(&self.streaming_in_progress, false);
            self.clear_pending_stream_buffers();
            self.set_query_edit_backup(None);
            self.table.redraw();
            return;
        }
        if let Some(session) = edit_session_snapshot {
            self.stage_query_edit_backup_from_current_state(session);
        } else {
            // A brand-new query started without an active edit session.
            // Drop any stale backup from older interrupted runs so an unrelated
            // failure result cannot resurrect obsolete staged edits.
            self.set_query_edit_backup(None);
            Self::clear_active_inline_edit_widget(&self.active_inline_edit);
        }
        self.clear_sort_state();

        *self
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let col_count = headers.len() as i32;

        // Clear any pending data from previous queries
        self.clear_pending_stream_buffers();
        self.full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        mutex_store_u64(&self.last_flush_epoch_ms, Self::current_epoch_millis());
        mutex_store_usize(&self.width_sampled_rows, 0);
        self.source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        // Edit mode is cleared at query start so the incoming result set is shown
        // as a fresh, non-edit session until explicitly re-enabled.
        let edit_mode_enabled = self
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some();
        // Detect the ROWID column up-front so it stays hidden throughout streaming,
        // not only after set_source_sql() is called at the end.
        *self
            .hidden_auto_rowid_col
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Self::detect_auto_hidden_rowid_col(headers, "", edit_mode_enabled);

        // Initialize pending widths based on headers
        let font_size = self.font_settings.size();
        let initial_widths: Vec<i32> = headers
            .iter()
            .map(|h| Self::estimate_text_width(h, font_size))
            .collect();
        *self
            .pending_widths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = initial_widths.clone();

        self.set_table_rows_for_current_font(0);
        self.table.set_cols(col_count);

        for (i, _name) in headers.iter().enumerate() {
            self.table.set_col_width(i as i32, initial_widths[i]);
        }
        self.apply_hidden_rowid_column_width();

        *self
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = headers.to_vec();
        self.table.redraw();
    }

    /// Append rows and flush immediately so each progress message is rendered.
    pub fn append_rows(&mut self, mut rows: Vec<Vec<String>>) {
        if self.is_save_pending() {
            return;
        }
        let lazy_fetch_session_id = if rows.is_empty() {
            None
        } else {
            *self
                .lazy_fetch_session
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        };
        if let Some(session_id) = lazy_fetch_session_id {
            self.lazy_fetch_more_in_flight
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&session_id);
        }

        // Only compute column widths for the first WIDTH_SAMPLE_ROWS rows.
        // After that threshold the sampling path is skipped entirely to avoid
        // locking pending_widths and iterating rows on the UI thread.
        let sampled = mutex_load_usize(&self.width_sampled_rows);
        let mut updated_sampled_rows = None;
        if sampled < WIDTH_SAMPLE_ROWS {
            let remaining = WIDTH_SAMPLE_ROWS - sampled;
            let sample_count = rows.len().min(remaining);
            let max_cols = rows[..sample_count]
                .iter()
                .map(|row| row.len())
                .max()
                .unwrap_or(0);
            let mut widths = self
                .pending_widths
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let min_width = Self::min_col_width_for_font(self.font_settings.size());
            let max_cell_display_chars = *self
                .max_cell_display_chars
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if widths.len() < max_cols {
                widths.resize(max_cols, min_width);
            }
            for row in rows[..sample_count].iter() {
                Self::update_widths_with_row(
                    &mut widths,
                    row,
                    self.font_settings.size(),
                    max_cell_display_chars,
                );
            }
            drop(widths);
            updated_sampled_rows = Some(sampled + sample_count);
        }

        // Add rows to pending buffer
        self.pending_rows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .append(&mut rows);

        self.flush_pending();
        if let Some(sampled_rows) = updated_sampled_rows {
            mutex_store_usize(&self.width_sampled_rows, sampled_rows);
        }
        if let Some(session_id) = lazy_fetch_session_id {
            if Self::is_table_near_bottom(&self.table) {
                Self::request_lazy_fetch_more_for_session(
                    session_id,
                    &self.lazy_fetch_session,
                    &self.lazy_fetch_callback,
                    &self.lazy_fetch_more_in_flight,
                );
            } else {
                self.drag_state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .vertical_scrollbar_fetch_requested = false;
            }
        }
    }

    /// Flush all pending rows to the UI.
    /// Data is moved (not cloned) from pending_rows into full_data.
    /// Only the table row count is updated — draw_cell handles rendering on demand.
    pub fn flush_pending(&mut self) {
        if self.is_save_pending() {
            self.pending_rows
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear();
            return;
        }

        let rows_to_add: Vec<Vec<String>> = {
            let mut pending_rows = self
                .pending_rows
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::take(&mut *pending_rows)
        };
        if rows_to_add.is_empty() {
            return;
        }

        let new_rows_count = rows_to_add.len() as i32;
        let current_rows = self.table.rows();
        let new_total = current_rows + new_rows_count;

        // Update column widths only while sampling is still active.
        // Once WIDTH_SAMPLE_ROWS rows have been measured, column widths are
        // finalized and we skip the lock + per-column iteration entirely.
        let sampled = mutex_load_usize(&self.width_sampled_rows);
        let should_apply_sampled_widths = sampled < WIDTH_SAMPLE_ROWS;

        // Move data into full_data — zero-copy, no clone!
        self.full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend(rows_to_add);

        // Just update row count — draw_cell reads from full_data on demand
        self.set_table_rows_for_current_font(new_total);

        if should_apply_sampled_widths {
            {
                let widths = self
                    .pending_widths
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let max_cols = widths.len().max(self.table.cols() as usize);
                if max_cols as i32 > self.table.cols() {
                    self.table.set_cols(max_cols as i32);
                }
                for (col_idx, &width) in widths.iter().enumerate() {
                    if col_idx < max_cols {
                        let current_width = self.table.col_width(col_idx as i32);
                        if width > current_width {
                            self.table.set_col_width(col_idx as i32, width);
                        }
                    }
                }
            }
            self.apply_hidden_rowid_column_width();
        }

        mutex_store_u64(&self.last_flush_epoch_ms, Self::current_epoch_millis());
        self.table.redraw();
    }

    /// Call this when streaming is complete to flush any remaining buffered rows
    pub fn finish_streaming(&mut self) {
        mutex_store_bool(&self.streaming_in_progress, false);
        *self
            .lazy_fetch_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.lazy_fetch_more_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        // flush_pending() already calls table.redraw() when rows are flushed.
        // Only issue an explicit redraw when the pending buffer was empty so the
        // streaming-complete state change is still rendered.
        let had_pending = !self
            .pending_rows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty();
        self.flush_pending();
        if !had_pending {
            self.table.redraw();
        }
    }

    pub fn set_lazy_fetch_callback(&mut self, callback: LazyFetchCallback) {
        *self
            .lazy_fetch_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Box::new(move |id, request| {
                ResultTableWidget::invoke_lazy_fetch_callback(&callback, id, request)
            }));
    }

    pub fn set_context_action_callback(&mut self, callback: ResultTableContextActionCallback) {
        let mut guard = self
            .context_action_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some(Box::new(move |action| {
            ResultTableWidget::invoke_context_action_callback(&callback, action);
        }));
    }

    pub fn set_lazy_fetch_session(&mut self, session_id: u64) {
        *self
            .lazy_fetch_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(session_id);
        self.lazy_fetch_more_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&session_id);
        mutex_store_bool(&self.streaming_in_progress, true);
    }

    /// A waiting report means the worker is idle, so a fetch-more still marked
    /// in flight can no longer produce a Rows event (an empty batch emits
    /// nothing); clear it so scroll-triggered fetches are not permanently
    /// gated.
    pub fn note_lazy_fetch_waiting(&mut self, session_id: u64) {
        Self::clear_lazy_fetch_more_in_flight(&self.lazy_fetch_more_in_flight, session_id);
    }

    fn clear_lazy_fetch_more_in_flight(in_flight: &Arc<Mutex<HashSet<u64>>>, session_id: u64) {
        in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&session_id);
    }

    pub fn clear_lazy_fetch_session(&mut self, session_id: u64, run_pending: bool) {
        let mut guard = self
            .lazy_fetch_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *guard == Some(session_id) {
            *guard = None;
        }
        drop(guard);
        self.lazy_fetch_more_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&session_id);
        if run_pending {
            self.run_pending_lazy_actions(session_id);
        } else {
            self.clear_pending_lazy_actions_for_session(session_id);
        }
    }

    pub fn clear_lazy_fetch_state_for_abort(&mut self) {
        *self
            .lazy_fetch_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.lazy_fetch_more_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.pending_lazy_actions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    pub fn active_lazy_fetch_session(&self) -> Option<u64> {
        *self
            .lazy_fetch_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn queue_action_after_fetch_all(&self, action: LazyFetchPendingAction) -> bool {
        Self::queue_lazy_action_if_active(
            &self.lazy_fetch_session,
            &self.lazy_fetch_callback,
            &self.pending_lazy_actions,
            action,
        )
    }

    fn apply_header_sort_action(&mut self, col_idx: usize) {
        let next_state = {
            let current = *self
                .sort_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::next_sort_state(current, col_idx)
        };
        if Self::apply_sort_to_table_data(
            &self.full_data,
            &self.edit_session,
            col_idx,
            next_state.direction,
        ) {
            *self
                .sort_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(next_state);
            self.table.redraw();
        }
    }

    fn clear_pending_lazy_actions_for_session(&self, session_id: u64) {
        let mut guard = self
            .pending_lazy_actions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.retain(|(queued_session_id, _)| *queued_session_id != session_id);
    }

    fn run_pending_lazy_actions(&mut self, session_id: u64) {
        let actions = {
            let mut guard = self
                .pending_lazy_actions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut actions = Vec::new();
            let mut retained = VecDeque::new();
            while let Some((queued_session_id, action)) = guard.pop_front() {
                if queued_session_id == session_id {
                    actions.push(action);
                } else {
                    retained.push_back((queued_session_id, action));
                }
            }
            *guard = retained;
            actions
        };
        for action in actions {
            match action {
                LazyFetchPendingAction::HeaderSort(col_idx) => {
                    self.apply_header_sort_action(col_idx)
                }
                LazyFetchPendingAction::CopyAll => {
                    let hidden_col = self.hidden_auto_rowid_col_value();
                    Self::copy_all_to_clipboard(&self.headers, &self.full_data, hidden_col);
                }
                LazyFetchPendingAction::SelectAll => self.select_all(),
                LazyFetchPendingAction::MoveToEdge { edge, selection } => {
                    let hidden_col = self.hidden_auto_rowid_col_value();
                    let selection = selection.unwrap_or_else(|| self.table.get_selection());
                    let col_position = Some(self.table.col_position());
                    Self::move_table_selection_to_edge_from_selection(
                        &mut self.table,
                        selection,
                        edge,
                        hidden_col,
                        col_position,
                    );
                    Self::sync_drag_selection_snapshot(&self.table, &self.drag_state);
                }
                LazyFetchPendingAction::ExtendSelectionToEdge { edge, selection } => {
                    let hidden_col = self.hidden_auto_rowid_col_value();
                    let selection = selection.unwrap_or_else(|| self.table.get_selection());
                    let col_position = Some(self.table.col_position());
                    if let Some(next_selection) =
                        Self::extend_table_selection_to_edge_from_selection(
                            &mut self.table,
                            selection,
                            edge,
                            hidden_col,
                            col_position,
                        )
                    {
                        Self::store_drag_selection_snapshot(&self.drag_state, Some(next_selection));
                    }
                }
                LazyFetchPendingAction::ExportCsv(mut callback) => {
                    let (csv, row_count) = self.current_csv_snapshot();
                    callback(csv, row_count);
                }
            }
        }
    }

    /// Recover from an interrupted edit-save batch that ended without a final
    /// statement result (for example, immediate query cancellation). Returns
    /// true when a stale pending-save flag was cleared.
    pub fn clear_orphaned_save_request(&mut self) -> bool {
        let mut pending = self
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !*pending {
            return false;
        }
        *pending = false;
        drop(pending);
        *self
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.pending_save_statement_signatures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        mutex_store_bool(&self.streaming_in_progress, false);
        self.clear_pending_stream_buffers();
        // Save orphan recovery should not leave stale pre-query snapshots that
        // can be resurrected by a later unrelated batch-finished cleanup.
        self.set_query_edit_backup(None);
        Self::clear_active_inline_edit_widget(&self.active_inline_edit);
        self.refresh_auto_rowid_visibility();
        self.table.redraw();
        true
    }

    /// Recover an edit session that was stashed at select-stream start but
    /// never finalized by a statement result (for example, abrupt cancellation).
    pub fn clear_orphaned_query_edit_backup(&mut self) -> bool {
        if *self
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            return false;
        }
        if self
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
        {
            return false;
        }
        // Drop any buffered stream rows from the interrupted query before
        // restoring the backed-up edit dataset.
        mutex_store_bool(&self.streaming_in_progress, false);
        self.clear_pending_stream_buffers();
        Self::clear_active_inline_edit_widget(&self.active_inline_edit);
        self.restore_query_edit_backup()
    }

    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.clear_lazy_fetch_state_for_abort();
        Self::clear_active_inline_edit_widget(&self.active_inline_edit);
        self.set_query_edit_backup(None);
        self.clear_sort_state();
        *self
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.table.set_rows(0);
        self.table.set_cols(0);
        {
            let mut headers = self
                .headers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            headers.clear();
            headers.shrink_to_fit();
        }
        {
            let mut pending_rows = self
                .pending_rows
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            pending_rows.clear();
            pending_rows.shrink_to_fit();
        }
        {
            let mut pending_widths = self
                .pending_widths
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            pending_widths.clear();
            pending_widths.shrink_to_fit();
        }
        {
            let mut full_data = self
                .full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            full_data.clear();
            full_data.shrink_to_fit();
        }
        mutex_store_usize(&self.width_sampled_rows, 0);
        mutex_store_u64(&self.last_flush_epoch_ms, Self::current_epoch_millis());
        *self
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        *self
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.pending_save_statement_signatures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        *self
            .hidden_auto_rowid_col
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.table.redraw();
    }

    pub fn copy(&self) -> usize {
        let Some((row_top, col_left, row_bot, col_right)) =
            Self::normalized_selection_bounds_with_limits(
                self.table.get_selection(),
                self.table.rows().max(0) as usize,
                self.table.cols().max(0) as usize,
            )
        else {
            return 0;
        };
        let hidden_col = self.hidden_auto_rowid_col_value();
        let count = Self::copy_selected_to_clipboard(&self.table, &self.full_data, hidden_col);
        if count > 0 {
            let rows = row_bot - row_top + 1;
            let cols = Self::visible_column_indices_in_range(col_left, col_right, hidden_col).len();
            println!("Copied {} cells ({} rows x {} cols)", count, rows, cols);
        }
        count
    }

    pub fn copy_with_headers(&self) {
        Self::copy_selected_with_headers(
            &self.table,
            &self.headers,
            &self.full_data,
            self.hidden_auto_rowid_col_value(),
        );
    }

    pub fn select_all(&mut self) {
        if self.queue_action_after_fetch_all(LazyFetchPendingAction::SelectAll) {
            return;
        }
        let rows = self.table.rows();
        let cols = self.table.cols();
        if rows > 0 && cols > 0 {
            self.table.set_selection(0, 0, rows - 1, cols - 1);
            Self::sync_drag_selection_snapshot(&self.table, &self.drag_state);
            self.table.redraw();
        }
    }

    #[doc(hidden)]
    pub(crate) fn capture_tour_select_range(
        &mut self,
        row_start: i32,
        col_start: i32,
        row_end: i32,
        col_end: i32,
    ) {
        self.table
            .set_selection(row_start, col_start, row_end, col_end);
        Self::sync_drag_selection_snapshot(&self.table, &self.drag_state);
        self.table.redraw();
    }

    pub fn paste_from_clipboard(&mut self) {
        let _ = self.table.take_focus();
        app::paste_text(&self.table);
    }

    #[allow(dead_code)]
    pub fn get_selected_data(&self) -> Option<String> {
        let (row_top, col_left, row_bot, col_right) =
            Self::normalized_selection_bounds_with_limits(
                self.table.get_selection(),
                self.table.rows().max(0) as usize,
                self.table.cols().max(0) as usize,
            )?;

        let full_data = self
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let rows = row_bot - row_top + 1;
        let hidden_col = self.hidden_auto_rowid_col_value();
        let visible_cols = Self::visible_column_indices_in_range(col_left, col_right, hidden_col);
        if visible_cols.is_empty() {
            return None;
        }
        let mut result = String::with_capacity(rows * visible_cols.len() * 16);
        for row in row_top..=row_bot {
            if row > row_top {
                result.push('\n');
            }
            for (visible_idx, col) in visible_cols.iter().enumerate() {
                if visible_idx > 0 {
                    result.push('\t');
                }
                if let Some(val) = full_data.get(row).and_then(|r| r.get(*col)) {
                    result.push_str(val);
                }
            }
        }

        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    fn build_csv_snapshot(&self) -> String {
        let line_ending = Self::csv_line_ending();
        let hidden_col = self.hidden_auto_rowid_col_value();
        let header_line = {
            let headers = self
                .headers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let escaped: Vec<String> = Self::visible_headers(&headers, hidden_col)
                .iter()
                .map(|h| Self::escape_csv_field(h))
                .collect();
            escaped.join(",")
        };

        let row_count = self
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len();
        let mut csv = String::with_capacity(row_count * 20 + header_line.len() + 4);

        // UTF-8 BOM so Excel detects the encoding and renders non-ASCII text
        // (e.g. Korean) correctly instead of falling back to the system locale.
        csv.push('\u{FEFF}');
        csv.push_str(&header_line);
        csv.push_str(line_ending);

        let full_data = self
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for row in full_data.iter() {
            let visible_row = Self::visible_row_values_internal(row, hidden_col);
            for (i, cell) in visible_row.iter().enumerate() {
                if i > 0 {
                    csv.push(',');
                }
                csv.push_str(&Self::escape_csv_field(cell));
            }
            csv.push_str(line_ending);
        }

        csv
    }

    /// Export all data to CSV format
    pub fn export_to_csv(&self) -> String {
        self.build_csv_snapshot()
    }

    pub fn export_to_csv_after_fetch_all(
        &self,
        callback: Box<dyn FnMut(String, usize)>,
    ) -> Option<(String, usize)> {
        if self.queue_action_after_fetch_all(LazyFetchPendingAction::ExportCsv(callback)) {
            None
        } else {
            Some(self.current_csv_snapshot())
        }
    }

    fn csv_line_ending() -> &'static str {
        if cfg!(windows) {
            "\r\n"
        } else {
            "\n"
        }
    }

    fn escape_csv_field(field: &str) -> String {
        if field.contains(',')
            || field.contains('"')
            || field.contains('\n')
            || field.contains('\r')
        {
            format!("\"{}\"", field.replace('"', "\"\""))
        } else {
            field.to_string()
        }
    }

    /// Escape a cell for tab-separated clipboard output so spreadsheet apps
    /// (Excel/Sheets) keep multiline and tab-containing values in a single cell.
    /// Fields containing a tab, newline, carriage return, or quote are wrapped
    /// in double quotes with embedded quotes doubled, matching the convention
    /// those apps use when parsing pasted TSV text.
    fn escape_cell_for_clipboard(field: &str) -> String {
        if field.contains('\t')
            || field.contains('\n')
            || field.contains('\r')
            || field.contains('"')
        {
            format!("\"{}\"", field.replace('"', "\"\""))
        } else {
            field.to_string()
        }
    }

    pub fn row_count(&self) -> usize {
        self.table.rows() as usize
    }

    pub fn has_data(&self) -> bool {
        self.table.rows() > 0
    }

    pub fn columns(&self) -> Vec<String> {
        let headers = self
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        Self::visible_headers(&headers, self.hidden_auto_rowid_col_value())
    }

    pub fn row_values(&self, row: usize) -> Option<Vec<String>> {
        self.full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(row)
            .map(|row_values| {
                Self::visible_row_values_internal(row_values, self.hidden_auto_rowid_col_value())
            })
    }

    pub fn get_widget(&self) -> Table {
        self.table.clone()
    }

    pub fn set_execute_sql_callback(&mut self, callback: Option<ResultGridSqlExecuteCallback>) {
        *self
            .execute_sql_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = callback;
    }

    pub fn set_null_text(&mut self, null_text: &str) {
        let normalized = null_text.to_string();
        *self
            .null_text
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = normalized.clone();
        let mut session_guard = self
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(session) = session_guard.as_mut() {
            let old_null_text = std::mem::replace(&mut session.null_text, normalized.clone());
            if old_null_text != normalized {
                let mut full_data = self
                    .full_data
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                for (row_idx, row_state) in session.row_states.iter().enumerate() {
                    let explicit_cols = Self::row_state_explicit_null_cols(row_state);
                    let Some(row) = full_data.get_mut(row_idx) else {
                        continue;
                    };
                    // Update explicit null cells: use value_represents_null
                    // (case-insensitive) instead of exact match so that
                    // user-typed variants like "null" are also updated.
                    for &col_idx in explicit_cols {
                        if col_idx < row.len()
                            && Self::value_represents_null(&row[col_idx], &old_null_text)
                        {
                            row[col_idx] = normalized.clone();
                        }
                    }
                    // Also update non-explicit null cells that still carry the
                    // executor's original null marker so that every null cell
                    // displays the newly configured null_text consistently.
                    for (col_idx, cell) in row.iter_mut().enumerate() {
                        if explicit_cols.contains(&col_idx) {
                            continue;
                        }
                        if Self::value_represents_null(cell.as_str(), &old_null_text) {
                            // Verify against the original snapshot: only rewrite
                            // the display value when the original was also null
                            // (avoids clobbering user-edited data that happens
                            // to look like a null marker).
                            if let EditRowState::Existing { rowid, .. } = row_state {
                                if let Some(orig) = session.original_rows_by_rowid.get(rowid) {
                                    let orig_val =
                                        orig.get(col_idx).map(|v| v.as_str()).unwrap_or("");
                                    if Self::value_represents_null(orig_val, &old_null_text) {
                                        *cell = normalized.clone();
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        self.table.redraw();
    }

    pub fn apply_font_settings(&mut self, profile: FontProfile, size: u32) {
        self.font_settings.update(profile, size);
        self.apply_table_metrics_for_current_font();
        self.table
            .set_row_height_all(Self::row_height_for_font(self.font_settings.size()));
        self.recalculate_widths_for_current_font();
        // Force FLTK to recalculate the table's internal layout after
        // header height / column width changes from the new font metrics.
        let (x, y, w, h) = (
            self.table.x(),
            self.table.y(),
            self.table.w(),
            self.table.h(),
        );
        self.table.resize(x, y, w, h);
        self.table.redraw();
    }

    #[cfg(test)]
    pub(crate) fn font_size_for_test(&self) -> u32 {
        self.font_settings.size()
    }

    pub fn set_max_cell_display_chars(&mut self, max_chars: usize) {
        let next = max_chars.max(1);
        *self
            .max_cell_display_chars
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
        self.max_cell_display_chars_draw
            .store(next, Ordering::Relaxed);
        self.recalculate_widths_for_current_font();
        self.table.redraw();
    }

    /// Cleanup method to release resources before the widget is deleted.
    pub fn cleanup(&mut self) {
        self.clear_lazy_fetch_state_for_abort();
        *self
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.set_query_edit_backup(None);
        self.clear_sort_state();
        Self::clear_active_inline_edit_widget(&self.active_inline_edit);

        // Clear callbacks to release captured Arc<Mutex<T>> references.
        self.table.handle(|_, _| false);
        self.table.resize_callback(|_, _, _, _, _| {});

        // Set an empty draw_cell to release captured Arc<Mutex<...>> references
        // from the virtual rendering callback.
        self.table.draw_cell(|_, _, _, _, _, _, _, _| {});

        // Reset table dimensions
        self.table.set_rows(0);
        self.table.set_cols(0);

        // Clear all data buffers to release memory
        {
            let mut headers = self
                .headers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            headers.clear();
            headers.shrink_to_fit();
        }
        {
            let mut pending_rows = self
                .pending_rows
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            pending_rows.clear();
            pending_rows.shrink_to_fit();
        }
        {
            let mut pending_widths = self
                .pending_widths
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            pending_widths.clear();
            pending_widths.shrink_to_fit();
        }
        {
            let mut full_data = self
                .full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            full_data.clear();
            full_data.shrink_to_fit();
        }
        self.source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        *self
            .hidden_auto_rowid_col
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .execute_sql_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        *self
            .active_inline_edit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

#[cfg(all(test, not(target_os = "linux")))]
mod row_edit_sql_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn sql_literal_from_input_handles_null_numbers_and_expr() {
        assert_eq!(
            ResultTableWidget::sql_literal_from_input(""),
            Ok("NULL".to_string())
        );
        assert_eq!(
            ResultTableWidget::sql_literal_from_input("NULL"),
            Ok("NULL".to_string())
        );
        assert_eq!(
            ResultTableWidget::sql_literal_from_input("42"),
            Ok("'42'".to_string())
        );
        assert_eq!(
            ResultTableWidget::sql_literal_from_input("3.14"),
            Ok("'3.14'".to_string())
        );
        assert_eq!(
            ResultTableWidget::sql_literal_from_input("00123"),
            Ok("'00123'".to_string())
        );
        assert_eq!(
            ResultTableWidget::sql_literal_from_input("=sysdate"),
            Ok("sysdate".to_string())
        );
        assert_eq!(
            ResultTableWidget::sql_literal_from_input("O'Reilly"),
            Ok("'O''Reilly'".to_string())
        );
    }

    #[test]
    fn sql_literal_from_input_preserves_significant_string_whitespace() {
        assert_eq!(
            ResultTableWidget::sql_literal_from_input("  padded  "),
            Ok("'  padded  '".to_string())
        );
        assert_eq!(
            ResultTableWidget::sql_literal_from_input(" = sysdate "),
            Ok("sysdate".to_string())
        );
    }

    #[test]
    fn sql_literal_from_input_rejects_expression_with_statement_or_comment_delimiters() {
        assert!(ResultTableWidget::sql_literal_from_input("=sysdate; delete from emp").is_err());
        assert!(ResultTableWidget::sql_literal_from_input("=sysdate --comment").is_err());
        assert!(ResultTableWidget::sql_literal_from_input("=/*hint*/sysdate").is_err());
    }

    #[test]
    fn find_rowid_column_index_accepts_qualified_header() {
        let headers = vec!["E.ROWID".to_string(), "ENAME".to_string()];
        assert_eq!(
            ResultTableWidget::find_rowid_column_index(&headers),
            Some(0)
        );
    }

    #[test]
    fn find_rowid_column_index_rejects_internal_rowid_alias_without_normalization() {
        let headers = vec!["SQ_INTERNAL_ROWID".to_string(), "ENAME".to_string()];
        assert_eq!(ResultTableWidget::find_rowid_column_index(&headers), None);
    }

    #[test]
    fn resolve_target_table_uses_rowid_alias_resolution() {
        let sql = "SELECT e.ROWID, e.ENAME, d.DNAME FROM EMP e JOIN DEPT d ON d.DEPTNO = e.DEPTNO";
        assert_eq!(
            ResultTableWidget::resolve_target_table(sql),
            Ok("EMP".to_string())
        );
    }

    #[test]
    fn resolve_target_table_uses_quoted_rowid_alias_resolution() {
        let sql = r#"SELECT "e"."ROWID", "e"."ENAME", "d"."DNAME" FROM EMP "e" JOIN DEPT "d" ON "d"."DEPTNO" = "e"."DEPTNO""#;
        assert_eq!(
            ResultTableWidget::resolve_target_table(sql),
            Ok("EMP".to_string())
        );
    }

    #[test]
    fn resolve_target_table_rejects_ambiguous_multi_table_without_rowid_alias() {
        let sql = "SELECT ROWID, e.ENAME, d.DNAME FROM EMP e JOIN DEPT d ON d.DEPTNO = e.DEPTNO";
        let result = ResultTableWidget::resolve_target_table(sql);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_target_table_resolves_join_with_qualified_rowid() {
        // This is the SQL after ROWID injection for a JOIN query
        let sql = "SELECT e.ROWID, e.ENAME, d.DNAME FROM EMP e JOIN DEPT d ON d.DEPTNO = e.DEPTNO";
        assert_eq!(
            ResultTableWidget::resolve_target_table(sql),
            Ok("EMP".to_string())
        );
    }

    #[test]
    fn resolve_target_table_resolves_comma_join_with_qualified_rowid() {
        let sql = "SELECT e.ROWID, ENAME FROM EMP e, DEPT d WHERE e.DEPTNO = d.DEPTNO";
        assert_eq!(
            ResultTableWidget::resolve_target_table(sql),
            Ok("EMP".to_string())
        );
    }

    #[test]
    fn resolve_target_table_resolves_with_clause_query() {
        let sql = "WITH dept_avg AS (SELECT DEPTNO, AVG(SAL) avg_sal FROM EMP GROUP BY DEPTNO) SELECT e.ROWID, ENAME FROM EMP e JOIN dept_avg d ON e.DEPTNO = d.DEPTNO";
        assert_eq!(
            ResultTableWidget::resolve_target_table(sql),
            Ok("EMP".to_string())
        );
    }

    #[test]
    fn resolve_target_table_resolves_left_join_with_qualified_rowid() {
        let sql =
            "SELECT e.ROWID, e.ENAME, d.DNAME FROM EMP e LEFT JOIN DEPT d ON e.DEPTNO = d.DEPTNO";
        assert_eq!(
            ResultTableWidget::resolve_target_table(sql),
            Ok("EMP".to_string())
        );
    }

    #[test]
    fn resolve_target_table_resolves_schema_qualified_table_with_alias() {
        let sql =
            "SELECT e.ROWID, e.ENAME FROM SCOTT.EMP e JOIN SCOTT.DEPT d ON e.DEPTNO = d.DEPTNO";
        assert_eq!(
            ResultTableWidget::resolve_target_table(sql),
            Ok("SCOTT.EMP".to_string())
        );
    }

    #[test]
    fn resolve_target_table_resolves_single_table_no_alias() {
        let sql = "SELECT EMP.ROWID, ENAME FROM EMP";
        assert_eq!(
            ResultTableWidget::resolve_target_table(sql),
            Ok("EMP".to_string())
        );
    }

    #[test]
    fn compose_edit_script_appends_source_select() {
        let script = ResultTableWidget::compose_edit_script(
            "UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AAA';",
            "SELECT ROWID, ENAME FROM EMP;",
        );
        assert_eq!(
            script,
            "UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AAA';\nSELECT ROWID, ENAME FROM EMP"
        );
    }

    #[test]
    fn last_identifier_segment_handles_qualified_and_quoted_identifiers() {
        assert_eq!(
            ResultTableWidget::last_identifier_segment("E.ENAME"),
            "ENAME"
        );
        assert_eq!(
            ResultTableWidget::last_identifier_segment("\"E\".\"EMP.NAME\""),
            "\"EMP.NAME\""
        );
        assert_eq!(
            ResultTableWidget::last_identifier_segment("[E].[EMP.NAME]"),
            "[EMP.NAME]"
        );
        assert_eq!(
            ResultTableWidget::last_identifier_segment("  ENAME  "),
            "ENAME"
        );
    }

    #[test]
    fn editable_column_identifier_uses_base_column_segment() {
        assert_eq!(
            ResultTableWidget::editable_column_identifier("E.ENAME"),
            Some("ENAME".to_string())
        );
        assert_eq!(
            ResultTableWidget::editable_column_identifier("\"E\".\"User Name\""),
            Some("\"User Name\"".to_string())
        );
        assert_eq!(
            ResultTableWidget::editable_column_identifier("SCOTT.\"A.B\""),
            Some("\"A.B\"".to_string())
        );
        assert_eq!(
            ResultTableWidget::editable_column_identifier("[E].[User.Name]"),
            Some("\"User.Name\"".to_string())
        );
        assert_eq!(
            ResultTableWidget::editable_column_identifier("[E].[User]]Name]"),
            Some("\"User]Name\"".to_string())
        );
        assert_eq!(ResultTableWidget::editable_column_identifier(""), None);
        assert_eq!(ResultTableWidget::editable_column_identifier("E."), None);
        assert_eq!(
            ResultTableWidget::editable_column_identifier("COUNT(*)"),
            None
        );
        assert_eq!(
            ResultTableWidget::editable_column_identifier("2ND_COL"),
            None
        );
        assert_eq!(
            ResultTableWidget::editable_column_identifier("\"BROKEN\"NAME\""),
            None
        );
        assert_eq!(
            ResultTableWidget::editable_column_identifier("[BROKEN"),
            None
        );
    }

    #[test]
    fn quote_qualified_identifier_preserves_dots_inside_quoted_segments() {
        assert_eq!(
            ResultTableWidget::quote_qualified_identifier(r#""SCHEMA.WITH.DOT"."TABLE.WITH.DOT""#),
            r#""SCHEMA.WITH.DOT"."TABLE.WITH.DOT""#
        );
        assert_eq!(
            ResultTableWidget::quote_qualified_identifier(r#""TABLE.WITH.DOT""#),
            r#""TABLE.WITH.DOT""#
        );
        assert_eq!(
            ResultTableWidget::quote_qualified_identifier("[SCHEMA.WITH.DOT].[TABLE.WITH.DOT]"),
            r#""SCHEMA.WITH.DOT"."TABLE.WITH.DOT""#
        );
        assert_eq!(
            ResultTableWidget::quote_qualified_identifier("[TABLE]]WITH]]BRACKET]"),
            r#""TABLE]WITH]BRACKET""#
        );
    }

    #[test]
    fn quote_qualified_identifier_keeps_unquoted_case_insensitive_identifiers_unquoted() {
        assert_eq!(
            ResultTableWidget::quote_qualified_identifier("scott.emp"),
            "scott.emp"
        );
        assert_eq!(
            ResultTableWidget::quote_qualified_identifier("MySchema.MyTable"),
            "MySchema.MyTable"
        );
    }

    #[test]
    fn push_unique_rowid_preserves_case_sensitive_values() {
        let mut rowids = Vec::new();
        let mut seen = HashSet::new();
        ResultTableWidget::push_unique_rowid(&mut rowids, &mut seen, "AAABbb");
        ResultTableWidget::push_unique_rowid(&mut rowids, &mut seen, "aaabbb");
        ResultTableWidget::push_unique_rowid(&mut rowids, &mut seen, " AAABbb ");
        assert_eq!(rowids, vec!["AAABbb".to_string(), "aaabbb".to_string()]);
    }

    #[test]
    fn resolve_update_target_cell_prefers_context_and_requires_single_selection_without_it() {
        assert_eq!(
            ResultTableWidget::resolve_update_target_cell((2, 3, 4, 5), 10, 10, Some((4, 5))),
            Some((4, 5))
        );
        assert_eq!(
            ResultTableWidget::resolve_update_target_cell((2, 3, 2, 3), 10, 10, None),
            Some((2, 3))
        );
        assert_eq!(
            ResultTableWidget::resolve_update_target_cell((2, 3, 4, 3), 10, 10, None),
            None
        );
    }

    #[test]
    fn single_cell_from_selection_accepts_only_native_single_cell_selection() {
        assert_eq!(
            ResultTableWidget::single_cell_from_selection((2, 3, 2, 3), 10, 10),
            Some((2, 3))
        );
        assert_eq!(
            ResultTableWidget::single_cell_from_selection((2, 0, 2, 9), 10, 10),
            None
        );
        assert_eq!(
            ResultTableWidget::single_cell_from_selection((0, 3, 9, 3), 10, 10),
            None
        );
    }

    #[test]
    fn resolved_selection_bounds_with_limits_clamps_to_current_table_size() {
        let bounds = ResultTableWidget::normalized_selection_bounds_with_limits((2, 3, 8, 9), 3, 4);
        assert_eq!(bounds, Some((2, 3, 2, 3)));
    }

    #[test]
    fn visible_column_bounds_skip_hidden_rowid_column() {
        assert_eq!(
            ResultTableWidget::visible_column_bounds(4, Some(0)),
            Some((1, 3))
        );
        assert_eq!(ResultTableWidget::visible_column_bounds(1, Some(0)), None);
    }

    #[test]
    fn selection_bounds_excluding_hidden_column_moves_single_hidden_cell_to_first_visible() {
        assert_eq!(
            ResultTableWidget::selection_bounds_excluding_hidden_column(
                (0, 0, 0, 0),
                3,
                4,
                Some(0)
            ),
            Some((0, 1, 0, 1))
        );
    }

    #[test]
    fn selection_bounds_excluding_hidden_column_trims_hidden_column_from_range() {
        assert_eq!(
            ResultTableWidget::selection_bounds_excluding_hidden_column(
                (1, 0, 1, 3),
                4,
                4,
                Some(0)
            ),
            Some((1, 1, 1, 3))
        );
    }

    #[test]
    fn oriented_selection_for_left_edge_keeps_left_side_as_moving_endpoint() {
        assert_eq!(
            ResultTableWidget::oriented_selection_for_edge((2, 1, 5, 3), ResultTableEdge::Left),
            (2, 3, 5, 1)
        );
    }

    #[test]
    fn oriented_selection_with_limits_preserves_direction_while_clamping() {
        assert_eq!(
            ResultTableWidget::oriented_selection_with_limits((8, 3, 2, 1), 5, 3),
            Some((4, 2, 2, 1))
        );
    }

    #[test]
    fn ctrl_shift_up_after_down_preserves_vertical_anchor() {
        assert_eq!(
            ResultTableWidget::oriented_selection_to_edge(
                (5, 2, 7, 2),
                10,
                4,
                None,
                ResultTableEdge::Up,
            ),
            Some((5, 2, 0, 2))
        );
    }

    #[test]
    fn ctrl_shift_down_after_up_preserves_vertical_anchor() {
        assert_eq!(
            ResultTableWidget::oriented_selection_to_edge(
                (5, 2, 0, 2),
                10,
                4,
                None,
                ResultTableEdge::Down,
            ),
            Some((5, 2, 9, 2))
        );
    }

    #[test]
    fn ctrl_shift_vertical_edges_toggle_around_initial_row_anchor() {
        let after_down = ResultTableWidget::oriented_selection_to_edge(
            (2, 1, 2, 1),
            10,
            4,
            None,
            ResultTableEdge::Down,
        );
        assert_eq!(after_down, Some((2, 1, 9, 1)));

        let after_up = ResultTableWidget::oriented_selection_to_edge(
            after_down.unwrap(),
            10,
            4,
            None,
            ResultTableEdge::Up,
        );
        assert_eq!(after_up, Some((2, 1, 0, 1)));

        assert_eq!(
            ResultTableWidget::oriented_selection_to_edge(
                after_up.unwrap(),
                10,
                4,
                None,
                ResultTableEdge::Down,
            ),
            Some((2, 1, 9, 1))
        );
    }

    #[test]
    fn vertical_edges_preserve_previous_horizontal_scroll_position() {
        assert_eq!(
            ResultTableWidget::preserved_col_position_for_edge(ResultTableEdge::Up, Some(7)),
            Some(7)
        );
        assert_eq!(
            ResultTableWidget::preserved_col_position_for_edge(ResultTableEdge::Down, Some(7)),
            Some(7)
        );
    }

    #[test]
    fn horizontal_edges_do_not_preserve_previous_horizontal_scroll_position() {
        assert_eq!(
            ResultTableWidget::preserved_col_position_for_edge(ResultTableEdge::Left, Some(7)),
            None
        );
        assert_eq!(
            ResultTableWidget::preserved_col_position_for_edge(ResultTableEdge::Right, Some(7)),
            None
        );
    }

    #[test]
    fn ctrl_shift_vertical_edge_preserves_column_direction() {
        assert_eq!(
            ResultTableWidget::oriented_selection_to_edge(
                (5, 3, 7, 1),
                10,
                4,
                None,
                ResultTableEdge::Down,
            ),
            Some((5, 3, 9, 1))
        );
    }

    #[test]
    fn ctrl_shift_vertical_edge_maps_hidden_rowid_endpoint_to_first_visible_column() {
        assert_eq!(
            ResultTableWidget::oriented_selection_to_edge(
                (5, 0, 5, 0),
                10,
                4,
                Some(0),
                ResultTableEdge::Down,
            ),
            Some((5, 1, 9, 1))
        );
    }

    #[test]
    fn hidden_left_boundary_shift_restore_keeps_previous_multirow_selection() {
        assert_eq!(
            ResultTableWidget::hidden_left_boundary_shift_restore_bounds(
                Some((2, 1, 5, 3)),
                (5, 0, 5, 0),
                8,
                4,
                Some(0),
                Key::Left,
                Key::Left,
            ),
            Some((2, 1, 5, 3))
        );
    }

    #[test]
    fn hidden_left_boundary_shift_restore_ignores_visible_right_edge_shrink() {
        assert_eq!(
            ResultTableWidget::hidden_left_boundary_shift_restore_bounds(
                Some((2, 1, 5, 3)),
                (2, 1, 5, 2),
                8,
                4,
                Some(0),
                Key::Left,
                Key::Left,
            ),
            None
        );
    }

    #[test]
    fn hidden_left_boundary_shift_restore_ignores_non_boundary_selection() {
        assert_eq!(
            ResultTableWidget::hidden_left_boundary_shift_restore_bounds(
                Some((2, 2, 5, 3)),
                (2, 1, 5, 3),
                8,
                4,
                Some(0),
                Key::Left,
                Key::Left,
            ),
            None
        );
    }

    #[test]
    fn boundary_arrow_uses_first_visible_column_when_rowid_is_hidden() {
        assert!(
            ResultTableWidget::should_consume_boundary_arrow_for_selection(
                (0, 1, 0, 1),
                3,
                4,
                Some(0),
                Key::Left,
            )
        );
        assert!(
            !ResultTableWidget::should_consume_boundary_arrow_for_selection(
                (0, 1, 0, 1),
                3,
                4,
                Some(0),
                Key::Right,
            )
        );
    }

    #[test]
    fn ctrl_left_target_uses_first_visible_column_when_rowid_is_hidden() {
        assert_eq!(
            ResultTableWidget::target_cell_for_edge(
                (2, 3, 2, 3),
                5,
                4,
                Some(0),
                ResultTableEdge::Left,
                0,
                0,
            ),
            Some((2, 1))
        );
    }

    #[test]
    fn ctrl_down_target_preserves_visible_column_after_hidden_rowid_selection() {
        assert_eq!(
            ResultTableWidget::target_cell_for_edge(
                (2, 0, 2, 0),
                6,
                4,
                Some(0),
                ResultTableEdge::Down,
                0,
                0,
            ),
            Some((5, 1))
        );
    }

    #[test]
    fn ctrl_shift_left_extension_stops_at_first_visible_column_when_rowid_is_hidden() {
        assert_eq!(
            ResultTableWidget::extended_selection_bounds_to_edge(
                (2, 3, 4, 3),
                6,
                4,
                Some(0),
                ResultTableEdge::Left,
            ),
            Some((2, 1, 4, 3))
        );
    }

    #[test]
    fn ctrl_shift_down_extension_keeps_current_columns_and_extends_to_last_row() {
        assert_eq!(
            ResultTableWidget::extended_selection_bounds_to_edge(
                (2, 1, 4, 2),
                8,
                4,
                Some(0),
                ResultTableEdge::Down,
            ),
            Some((2, 1, 7, 2))
        );
    }

    #[test]
    fn down_edge_row_position_keeps_last_row_visible_without_overshooting_scroll_range() {
        assert_eq!(
            ResultTableWidget::bounded_row_position_for_edge_target(
                99,
                100,
                20,
                ResultTableEdge::Down,
            ),
            80
        );
    }

    #[test]
    fn down_edge_row_position_uses_zero_when_all_rows_are_visible() {
        assert_eq!(
            ResultTableWidget::bounded_row_position_for_edge_target(
                7,
                8,
                20,
                ResultTableEdge::Down,
            ),
            0
        );
    }

    #[test]
    fn down_edge_row_position_uses_full_visible_rows_for_bottom_alignment() {
        assert_eq!(
            ResultTableWidget::bounded_row_position_for_edge_target(
                9999,
                10000,
                19,
                ResultTableEdge::Down,
            ),
            9981
        );
    }

    #[test]
    fn up_edge_row_position_keeps_target_row_as_view_start() {
        assert_eq!(
            ResultTableWidget::bounded_row_position_for_edge_target(
                42,
                100,
                20,
                ResultTableEdge::Up,
            ),
            42
        );
    }

    #[test]
    fn normalized_selection_bounds_with_limits_rejects_no_overlap_selection() {
        let bounds =
            ResultTableWidget::normalized_selection_bounds_with_limits((10, 0, 11, 1), 5, 5);
        assert_eq!(bounds, None);
    }

    #[test]
    fn resolve_update_target_cell_rejects_out_of_range_context_cell() {
        assert_eq!(
            ResultTableWidget::resolve_update_target_cell((2, 3, 2, 3), 1, 1, Some((3, 0))),
            None
        );
    }

    #[test]
    fn is_staged_cell_modified_for_existing_row_compares_against_original() {
        let mut original_rows = HashMap::new();
        original_rows.insert(
            "RID1".to_string(),
            vec!["RID1".to_string(), "OLD".to_string(), "X".to_string()],
        );
        let session = TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "C1".to_string()), (2, "C2".to_string())],
            original_rows_by_rowid: original_rows,
            original_row_order: vec!["RID1".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "RID1".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        };
        let current_row = vec!["RID1".to_string(), "NEW".to_string(), "X".to_string()];

        assert!(ResultTableWidget::is_staged_cell_modified(
            &session,
            0,
            1,
            &current_row
        ));
        assert!(!ResultTableWidget::is_staged_cell_modified(
            &session,
            0,
            2,
            &current_row
        ));
    }

    #[test]
    fn sync_existing_row_dirty_cell_tracks_dirty_flag_for_draw_fast_path() {
        let mut original_rows = HashMap::new();
        original_rows.insert(
            "RID1".to_string(),
            vec!["RID1".to_string(), "OLD".to_string()],
        );
        let mut session = TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "C1".to_string())],
            original_rows_by_rowid: original_rows,
            original_row_order: vec!["RID1".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "RID1".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        };

        ResultTableWidget::sync_existing_row_dirty_cell(&mut session, 0, 1, "NEW");
        let is_dirty = session
            .row_states
            .first()
            .and_then(|row_state| match row_state {
                EditRowState::Existing { dirty_cols, .. } => Some(dirty_cols.contains(&1)),
                EditRowState::Inserted { .. } => None,
            })
            .unwrap_or(false);
        assert!(is_dirty);

        let dirty_row = vec!["RID1".to_string(), "NEW".to_string()];
        assert!(ResultTableWidget::cell_edit_state_for_draw(&session, 0, 1, &dirty_row).0);

        ResultTableWidget::sync_existing_row_dirty_cell(&mut session, 0, 1, "OLD");
        let is_dirty = session
            .row_states
            .first()
            .and_then(|row_state| match row_state {
                EditRowState::Existing { dirty_cols, .. } => Some(dirty_cols.contains(&1)),
                EditRowState::Inserted { .. } => None,
            })
            .unwrap_or(false);
        assert!(!is_dirty);

        let clean_row = vec!["RID1".to_string(), "OLD".to_string()];
        assert!(!ResultTableWidget::cell_edit_state_for_draw(&session, 0, 1, &clean_row).0);
    }

    #[test]
    fn is_staged_cell_modified_for_inserted_row_highlights_non_empty_editable_cells() {
        let session = TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "C1".to_string()), (2, "C2".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Inserted {
                explicit_null_cols: HashSet::new(),
            }],
        };
        let current_row = vec!["".to_string(), "VALUE".to_string(), "".to_string()];

        assert!(ResultTableWidget::is_staged_cell_modified(
            &session,
            0,
            1,
            &current_row
        ));
        assert!(!ResultTableWidget::is_staged_cell_modified(
            &session,
            0,
            2,
            &current_row
        ));
    }

    #[test]
    fn is_staged_cell_modified_treats_explicit_null_as_modified_even_when_text_matches() {
        let mut original_rows = HashMap::new();
        original_rows.insert(
            "RID1".to_string(),
            vec!["RID1".to_string(), "NULL".to_string()],
        );
        let session = TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "C1".to_string())],
            original_rows_by_rowid: original_rows,
            original_row_order: vec!["RID1".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "RID1".to_string(),
                explicit_null_cols: [1usize].into_iter().collect(),
                dirty_cols: HashSet::new(),
            }],
        };
        let current_row = vec!["RID1".to_string(), "NULL".to_string()];

        assert!(ResultTableWidget::is_staged_cell_modified(
            &session,
            0,
            1,
            &current_row
        ));
    }

    #[test]
    fn row_cell_is_original_null_returns_true_for_existing_null_cell() {
        let mut original_rows = HashMap::new();
        original_rows.insert(
            "RID1".to_string(),
            vec!["RID1".to_string(), "NULL".to_string()],
        );
        let session = TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "C1".to_string())],
            original_rows_by_rowid: original_rows,
            original_row_order: vec!["RID1".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "RID1".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        };
        let current_row = vec!["RID1".to_string(), "NULL".to_string()];

        assert!(ResultTableWidget::row_cell_is_original_null(
            &session,
            0,
            1,
            &current_row
        ));
    }

    #[test]
    fn row_cell_is_original_null_returns_false_when_value_changed_from_null() {
        let mut original_rows = HashMap::new();
        original_rows.insert(
            "RID1".to_string(),
            vec!["RID1".to_string(), "NULL".to_string()],
        );
        let session = TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "C1".to_string())],
            original_rows_by_rowid: original_rows,
            original_row_order: vec!["RID1".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "RID1".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        };
        let current_row = vec!["RID1".to_string(), "SMITH".to_string()];

        assert!(!ResultTableWidget::row_cell_is_original_null(
            &session,
            0,
            1,
            &current_row
        ));
    }

    #[test]
    fn input_maps_to_explicit_null_uses_configured_null_text_marker() {
        let row_state = EditRowState::Existing {
            rowid: "RID1".to_string(),
            explicit_null_cols: HashSet::new(),
            dirty_cols: HashSet::new(),
        };

        assert!(ResultTableWidget::input_maps_to_explicit_null(
            &row_state, "(null)", "(null)"
        ));
        assert!(!ResultTableWidget::input_maps_to_explicit_null(
            &row_state, "NULL", "(null)"
        ));
        assert!(ResultTableWidget::input_maps_to_explicit_null(
            &row_state, "=NULL", "(null)"
        ));
    }

    #[test]
    fn sql_literal_from_input_with_null_text_respects_custom_marker() {
        assert_eq!(
            ResultTableWidget::sql_literal_from_input_with_null_text("(null)", "(null)"),
            Ok("NULL".to_string())
        );
        assert_eq!(
            ResultTableWidget::sql_literal_from_input_with_null_text("null", "(null)"),
            Ok("'null'".to_string())
        );
    }

    #[test]
    fn sql_literal_from_input_with_null_text_treats_whitespace_as_string_literal() {
        assert_eq!(
            ResultTableWidget::sql_literal_from_input_with_null_text("   ", "NULL"),
            Ok("'   '".to_string())
        );
        assert_eq!(
            ResultTableWidget::sql_literal_from_input_with_null_text("	", "NULL"),
            Ok("'	'".to_string())
        );
    }

    #[test]
    fn input_maps_to_explicit_null_does_not_treat_whitespace_as_null() {
        let row_state = EditRowState::Existing {
            rowid: "RID1".to_string(),
            explicit_null_cols: HashSet::new(),
            dirty_cols: HashSet::new(),
        };

        assert!(!ResultTableWidget::input_maps_to_explicit_null(
            &row_state, "   ", "NULL"
        ));
        assert!(!ResultTableWidget::input_maps_to_explicit_null(
            &row_state, "	", "NULL"
        ));
    }

    #[test]
    fn value_represents_null_does_not_treat_whitespace_as_null() {
        assert!(!ResultTableWidget::value_represents_null("   ", "NULL"));
        assert!(!ResultTableWidget::value_represents_null("	", "NULL"));
        assert!(ResultTableWidget::value_represents_null("", "NULL"));
    }

    #[test]
    fn row_cell_is_original_null_recognizes_executor_null_with_custom_null_text() {
        let mut original_rows = HashMap::new();
        original_rows.insert(
            "RID1".to_string(),
            vec!["RID1".to_string(), "NULL".to_string()],
        );
        // null_text is custom "(null)" but the executor stores DB NULLs as "NULL".
        let session = TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "(null)".to_string(),
            editable_columns: vec![(1, "C1".to_string())],
            original_rows_by_rowid: original_rows,
            original_row_order: vec!["RID1".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "RID1".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        };
        // Current value is still "NULL" from the executor (user hasn't edited).
        let current_row = vec!["RID1".to_string(), "NULL".to_string()];
        assert!(ResultTableWidget::row_cell_is_original_null(
            &session,
            0,
            1,
            &current_row
        ));
    }

    #[test]
    fn row_cell_is_original_null_detects_change_from_null_with_custom_null_text() {
        let mut original_rows = HashMap::new();
        original_rows.insert(
            "RID1".to_string(),
            vec!["RID1".to_string(), "NULL".to_string()],
        );
        let session = TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "(null)".to_string(),
            editable_columns: vec![(1, "C1".to_string())],
            original_rows_by_rowid: original_rows,
            original_row_order: vec!["RID1".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "RID1".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        };
        // User changed the cell value from NULL to actual data.
        let current_row = vec!["RID1".to_string(), "SMITH".to_string()];
        assert!(!ResultTableWidget::row_cell_is_original_null(
            &session,
            0,
            1,
            &current_row
        ));
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn set_null_text_updates_case_variant_explicit_null_cells() {
        let mut widget = ResultTableWidget::new();
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec!["ROWID".to_string(), "ENAME".to_string()];
        // User typed "null" (lowercase) which was accepted as explicit null.
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "null".to_string()]];
        let mut original_rows_by_rowid = HashMap::new();
        original_rows_by_rowid.insert(
            "AAABBB".to_string(),
            vec!["AAABBB".to_string(), "SMITH".to_string()],
        );
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid,
            original_row_order: vec!["AAABBB".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "AAABBB".to_string(),
                explicit_null_cols: [1usize].into_iter().collect(),
                dirty_cols: HashSet::new(),
            }],
        });

        // Changing null_text should update even the lowercase variant.
        widget.set_null_text("(null)");

        let data = widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert_eq!(data[0][1], "(null)");
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn set_null_text_updates_original_db_null_cells() {
        let mut widget = ResultTableWidget::new();
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec!["ROWID".to_string(), "ENAME".to_string()];
        // Executor stores DB NULL as "NULL".
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "NULL".to_string()]];
        let mut original_rows_by_rowid = HashMap::new();
        original_rows_by_rowid.insert(
            "AAABBB".to_string(),
            vec!["AAABBB".to_string(), "NULL".to_string()],
        );
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid,
            original_row_order: vec!["AAABBB".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "AAABBB".to_string(),
                // Not in explicit_null_cols — this is an untouched original null cell.
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        });

        // Changing null_text should also update non-explicit null cells
        // whose original value was null.
        widget.set_null_text("(null)");

        let data = widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert_eq!(data[0][1], "(null)");
    }

    #[test]
    fn selection_contains_cell_normalizes_reversed_bounds() {
        assert!(ResultTableWidget::selection_contains_cell(
            (5, 6, 2, 3),
            4,
            5
        ));
        assert!(ResultTableWidget::selection_contains_cell(
            (5, 6, 2, 3),
            2,
            3
        ));
        assert!(!ResultTableWidget::selection_contains_cell(
            (5, 6, 2, 3),
            1,
            3
        ));
    }

    #[test]
    fn selection_contains_cell_rejects_negative_or_empty_selection() {
        assert!(!ResultTableWidget::selection_contains_cell(
            (-1, -1, -1, -1),
            0,
            0
        ));
        assert!(!ResultTableWidget::selection_contains_cell(
            (0, 0, 1, 1),
            -1,
            0
        ));
    }

    #[test]
    fn oriented_selection_with_cell_keeps_shift_click_anchor() {
        let first =
            ResultTableWidget::oriented_selection_with_cell(Some((2, 2, 2, 2)), 1, 1, 10, 10);
        assert_eq!(first, Some((2, 2, 1, 1)));
        assert_eq!(
            first.and_then(ResultTableWidget::normalized_selection_bounds),
            Some((1, 1, 2, 2))
        );

        let second = ResultTableWidget::oriented_selection_with_cell(first, 3, 3, 10, 10);
        assert_eq!(second, Some((2, 2, 3, 3)));
        assert_eq!(
            second.and_then(ResultTableWidget::normalized_selection_bounds),
            Some((2, 2, 3, 3))
        );
    }

    #[test]
    fn oriented_selection_with_cell_keeps_anchor_after_reversing_direction() {
        let after_down =
            ResultTableWidget::oriented_selection_with_cell(Some((2, 2, 2, 2)), 3, 3, 10, 10)
                .expect("shift-click down-right selection");
        assert_eq!(after_down, (2, 2, 3, 3));

        let after_up =
            ResultTableWidget::oriented_selection_with_cell(Some(after_down), 1, 1, 10, 10)
                .expect("shift-click up-left selection");
        assert_eq!(after_up, (2, 2, 1, 1));
        assert_eq!(
            ResultTableWidget::normalized_selection_bounds(after_up),
            Some((1, 1, 2, 2))
        );

        let after_down_again =
            ResultTableWidget::oriented_selection_with_cell(Some(after_up), 3, 3, 10, 10)
                .expect("shift-click down-right again selection");
        assert_eq!(after_down_again, (2, 2, 3, 3));
        assert_eq!(
            ResultTableWidget::normalized_selection_bounds(after_down_again),
            Some((2, 2, 3, 3))
        );
    }

    #[test]
    fn shift_click_selection_with_cell_preserves_initial_multi_cell_base() {
        let base_selection = Some((2, 2, 3, 3));

        let after_up_left =
            ResultTableWidget::shift_click_selection_with_cell(base_selection, 1, 1, 10, 10)
                .expect("shift-click up-left from multi-cell base");
        assert_eq!(
            ResultTableWidget::normalized_selection_bounds(after_up_left),
            Some((1, 1, 3, 3))
        );

        let after_down_right =
            ResultTableWidget::shift_click_selection_with_cell(base_selection, 4, 4, 10, 10)
                .expect("shift-click down-right from same multi-cell base");
        assert_eq!(
            ResultTableWidget::normalized_selection_bounds(after_down_right),
            Some((2, 2, 4, 4))
        );
    }

    #[test]
    fn expanded_selection_bounds_with_cell_extends_existing_bounds_in_all_directions() {
        assert_eq!(
            ResultTableWidget::expanded_selection_bounds_with_cell(
                Some((2, 3, 4, 5)),
                6,
                7,
                10,
                10,
            ),
            Some((2, 3, 6, 7))
        );
        assert_eq!(
            ResultTableWidget::expanded_selection_bounds_with_cell(
                Some((2, 3, 4, 5)),
                1,
                2,
                10,
                10,
            ),
            Some((1, 2, 4, 5))
        );
        assert_eq!(
            ResultTableWidget::expanded_selection_bounds_with_cell(
                Some((2, 3, 4, 5)),
                1,
                7,
                10,
                10,
            ),
            Some((1, 3, 4, 7))
        );
        assert_eq!(
            ResultTableWidget::expanded_selection_bounds_with_cell(
                Some((2, 3, 4, 5)),
                6,
                2,
                10,
                10,
            ),
            Some((2, 2, 6, 5))
        );
    }

    #[test]
    fn expanded_selection_bounds_with_cell_uses_target_when_base_selection_is_empty() {
        assert_eq!(
            ResultTableWidget::expanded_selection_bounds_with_cell(None, 4, 6, 10, 10),
            Some((4, 6, 4, 6))
        );
    }

    #[test]
    fn expanded_selection_bounds_with_cell_rejects_out_of_range_target() {
        assert_eq!(
            ResultTableWidget::expanded_selection_bounds_with_cell(
                Some((2, 3, 4, 5)),
                -1,
                2,
                10,
                10
            ),
            None
        );
        assert_eq!(
            ResultTableWidget::expanded_selection_bounds_with_cell(
                Some((2, 3, 4, 5)),
                10,
                2,
                10,
                10
            ),
            None
        );
    }

    #[test]
    fn visible_row_metrics_rounds_up_partial_last_row() {
        assert_eq!(
            ResultTableWidget::visible_row_metrics(10, 61, 20),
            Some((3, 61))
        );
        assert_eq!(
            ResultTableWidget::visible_row_metrics(10, 50, 20),
            Some((2, 50))
        );
        assert_eq!(ResultTableWidget::visible_row_metrics(10, 10, 20), None);
    }

    #[test]
    fn offscreen_skip_count_scales_with_hidden_distance() {
        assert_eq!(
            ResultTableWidget::offscreen_skip_count(100, 20, 140),
            Some(1)
        );
        assert_eq!(
            ResultTableWidget::offscreen_skip_count(100, 20, 180),
            Some(3)
        );
    }

    #[test]
    fn offscreen_skip_count_rejects_visible_or_invalid_extent() {
        assert_eq!(ResultTableWidget::offscreen_skip_count(130, 20, 140), None);
        assert_eq!(ResultTableWidget::offscreen_skip_count(100, 0, 140), None);
    }

    #[test]
    fn estimate_uniform_row_candidate_uses_actual_visible_row_origin() {
        assert_eq!(
            ResultTableWidget::estimate_uniform_row_candidate(10, 102, 20, 121, 100),
            Some(10)
        );
        assert_eq!(
            ResultTableWidget::estimate_uniform_row_candidate(10, 102, 20, 122, 100),
            Some(11)
        );
    }

    #[test]
    fn estimate_uniform_row_candidate_rejects_invalid_geometry() {
        assert_eq!(
            ResultTableWidget::estimate_uniform_row_candidate(10, 102, 0, 121, 100),
            None
        );
        assert_eq!(
            ResultTableWidget::estimate_uniform_row_candidate(10, 102, 20, 121, 0),
            None
        );
        assert_eq!(
            ResultTableWidget::estimate_uniform_row_candidate(100, 102, 20, 121, 100),
            None
        );
    }

    #[test]
    fn is_mouse_within_bounds_includes_top_left_excludes_right_bottom() {
        assert!(ResultTableWidget::is_mouse_within_bounds(
            10, 20, 10, 20, 100, 40
        ));
        assert!(!ResultTableWidget::is_mouse_within_bounds(
            110, 20, 10, 20, 100, 40
        ));
        assert!(!ResultTableWidget::is_mouse_within_bounds(
            10, 60, 10, 20, 100, 40
        ));
    }

    #[test]
    fn is_mouse_within_bounds_rejects_non_positive_dimensions() {
        assert!(!ResultTableWidget::is_mouse_within_bounds(
            10, 20, 10, 20, 0, 40
        ));
        assert!(!ResultTableWidget::is_mouse_within_bounds(
            10, 20, 10, 20, 100, -1
        ));
    }

    #[test]
    fn background_pointer_sequence_is_consumed_for_blank_table_area() {
        assert!(
            ResultTableWidget::should_consume_background_pointer_sequence(
                true, false, None, None, None
            )
        );
        assert!(
            !ResultTableWidget::should_consume_background_pointer_sequence(
                false, false, None, None, None
            )
        );
        assert!(
            !ResultTableWidget::should_consume_background_pointer_sequence(
                true, true, None, None, None
            )
        );
    }

    #[test]
    fn background_pointer_sequence_is_not_consumed_for_real_table_targets() {
        assert!(
            !ResultTableWidget::should_consume_background_pointer_sequence(
                true,
                false,
                Some((4, 7)),
                None,
                None
            )
        );
        assert!(
            !ResultTableWidget::should_consume_background_pointer_sequence(
                true,
                false,
                None,
                Some(3),
                None
            )
        );
        assert!(
            !ResultTableWidget::should_consume_background_pointer_sequence(
                true,
                false,
                None,
                None,
                Some(2)
            )
        );
    }

    #[test]
    fn escape_cell_for_clipboard_quotes_multiline_and_special_chars() {
        // Plain values pass through untouched.
        assert_eq!(ResultTableWidget::escape_cell_for_clipboard("abc"), "abc");
        assert_eq!(ResultTableWidget::escape_cell_for_clipboard("a,b"), "a,b");
        // Newlines, carriage returns, tabs and quotes force quoting.
        assert_eq!(
            ResultTableWidget::escape_cell_for_clipboard("line1\nline2"),
            "\"line1\nline2\""
        );
        assert_eq!(
            ResultTableWidget::escape_cell_for_clipboard("a\tb"),
            "\"a\tb\""
        );
        assert_eq!(
            ResultTableWidget::escape_cell_for_clipboard("say \"hi\""),
            "\"say \"\"hi\"\"\""
        );
    }

    #[test]
    fn parse_clipboard_rows_round_trips_escaped_multiline_cell() {
        // A two-column row where the second cell spans multiple lines and
        // contains a tab and a quote, escaped as a spreadsheet would expect.
        let cell = "line1\nline2\twith \"quote\"";
        let escaped = ResultTableWidget::escape_cell_for_clipboard(cell);
        let clipboard = format!("A\t{}\n", escaped);
        let rows = ResultTableWidget::parse_clipboard_rows(&clipboard);
        assert_eq!(rows, vec![vec!["A".to_string(), cell.to_string()]]);
    }

    #[test]
    fn normalized_selection_bounds_reorders_reversed_selection() {
        assert_eq!(
            ResultTableWidget::normalized_selection_bounds((5, 6, 2, 3)),
            Some((2, 3, 5, 6))
        );
    }

    #[test]
    fn normalized_selection_bounds_rejects_negative_selection() {
        assert_eq!(
            ResultTableWidget::normalized_selection_bounds((-1, 6, 2, 3)),
            None
        );
        assert_eq!(
            ResultTableWidget::normalized_selection_bounds((2, 3, -1, 6)),
            None
        );
    }

    #[test]
    fn parse_clipboard_rows_normalizes_line_endings_and_trailing_newline() {
        let rows = ResultTableWidget::parse_clipboard_rows("A\tB\r\n1\t2\r\n");
        assert_eq!(
            rows,
            vec![
                vec!["A".to_string(), "B".to_string()],
                vec!["1".to_string(), "2".to_string()]
            ]
        );
    }

    #[test]
    fn apply_paste_values_to_data_fills_selection_for_single_value() {
        let mut data = vec![
            vec!["RID1".to_string(), "A".to_string(), "B".to_string()],
            vec!["RID2".to_string(), "C".to_string(), "D".to_string()],
        ];
        let editable_cols: HashSet<usize> = [1usize, 2usize].into_iter().collect();
        let changed = ResultTableWidget::apply_paste_values_to_data(
            &mut data,
            0,
            &editable_cols,
            3,
            (0, 1),
            Some((0, 1, 1, 2)),
            &[vec!["X".to_string()]],
        );
        assert_eq!((changed.0, changed.1), (4, 0));
        assert_eq!(changed.2.len(), 4);
        assert_eq!(
            data,
            vec![
                vec!["RID1".to_string(), "X".to_string(), "X".to_string()],
                vec!["RID2".to_string(), "X".to_string(), "X".to_string()],
            ]
        );
    }

    #[test]
    fn apply_paste_values_to_data_skips_rowid_and_non_editable_columns() {
        let mut data = vec![vec![
            "RID1".to_string(),
            "A".to_string(),
            "B".to_string(),
            "C".to_string(),
        ]];
        let editable_cols: HashSet<usize> = [1usize, 3usize].into_iter().collect();
        let changed = ResultTableWidget::apply_paste_values_to_data(
            &mut data,
            0,
            &editable_cols,
            4,
            (0, 0),
            None,
            &[vec![
                "R".to_string(),
                "X".to_string(),
                "Y".to_string(),
                "Z".to_string(),
            ]],
        );
        assert_eq!((changed.0, changed.1), (2, 0));
        assert_eq!(changed.2.len(), 2);
        assert_eq!(
            data,
            vec![vec![
                "RID1".to_string(),
                "X".to_string(),
                "B".to_string(),
                "Z".to_string(),
            ]]
        );
    }

    #[test]
    fn apply_paste_values_to_data_counts_cells_beyond_visible_columns_as_skipped() {
        let mut data = vec![vec![
            "RID1".to_string(),
            "A".to_string(),
            "B".to_string(),
            "C".to_string(),
        ]];
        let editable_cols: HashSet<usize> = [1usize, 2usize, 3usize].into_iter().collect();
        let changed = ResultTableWidget::apply_paste_values_to_data(
            &mut data,
            0,
            &editable_cols,
            3,
            (0, 2),
            None,
            &[vec!["X".to_string(), "Y".to_string()]],
        );
        assert_eq!((changed.0, changed.1), (1, 1));
        assert_eq!(changed.2.len(), 1);
        assert_eq!(
            data,
            vec![vec![
                "RID1".to_string(),
                "A".to_string(),
                "X".to_string(),
                "C".to_string(),
            ]]
        );
    }

    #[test]
    fn resolve_paste_anchor_column_prefers_editable_col_when_anchor_is_rowid() {
        let editable_cols: HashSet<usize> = [1usize, 2usize].into_iter().collect();
        let resolved = ResultTableWidget::resolve_paste_anchor_column(
            0,
            Some((0, 0, 0, 2)),
            0,
            &editable_cols,
            3,
        );
        assert_eq!(resolved, Some(1));
    }

    #[test]
    fn resolve_paste_anchor_column_keeps_anchor_when_already_editable() {
        let editable_cols: HashSet<usize> = [1usize, 3usize].into_iter().collect();
        let resolved = ResultTableWidget::resolve_paste_anchor_column(
            3,
            Some((0, 0, 0, 3)),
            0,
            &editable_cols,
            4,
        );
        assert_eq!(resolved, Some(3));
    }

    #[test]
    fn collect_rowids_in_range_errors_when_selected_row_lacks_rowid_cell() {
        let full_data = vec![vec!["AAABBB".to_string()], Vec::new()];
        let result = ResultTableWidget::collect_rowids_in_range(0, 1, 0, &full_data);
        assert!(result.is_err());
    }

    #[test]
    fn collect_rowids_in_range_errors_when_selected_row_has_empty_rowid() {
        let full_data = vec![vec!["AAABBB".to_string()], vec!["   ".to_string()]];
        let result = ResultTableWidget::collect_rowids_in_range(0, 1, 0, &full_data);
        assert!(result.is_err());
    }

    #[test]
    fn can_show_insert_row_action_requires_resolved_target() {
        assert!(ResultTableWidget::can_show_insert_row_action(
            "SELECT ENAME FROM EMP"
        ));
        assert!(!ResultTableWidget::can_show_insert_row_action("   "));
        // Unqualified ROWID in multi-table is ambiguous
        assert!(!ResultTableWidget::can_show_insert_row_action(
            "SELECT ROWID, e.ENAME, d.DNAME FROM EMP e JOIN DEPT d ON d.DEPTNO = e.DEPTNO"
        ));
        // JOIN result sets are not editable even with qualified ROWID.
        assert!(!ResultTableWidget::can_show_insert_row_action(
            "SELECT e.ROWID, e.ENAME, d.DNAME FROM EMP e JOIN DEPT d ON d.DEPTNO = e.DEPTNO"
        ));
    }

    #[test]
    fn can_show_rowid_edit_actions_requires_rowid_and_resolved_target() {
        let valid_headers = vec!["ROWID".to_string(), "ENAME".to_string()];
        assert!(ResultTableWidget::can_show_rowid_edit_actions(
            &valid_headers,
            "SELECT ROWID, ENAME FROM EMP"
        ));

        let missing_rowid_headers = vec!["ENAME".to_string()];
        assert!(!ResultTableWidget::can_show_rowid_edit_actions(
            &missing_rowid_headers,
            "SELECT ENAME FROM EMP"
        ));
        assert!(!ResultTableWidget::can_show_rowid_edit_actions(
            &valid_headers,
            "   "
        ));
        // Unqualified ROWID in multi-table is ambiguous
        assert!(!ResultTableWidget::can_show_rowid_edit_actions(
            &valid_headers,
            "SELECT ROWID, e.ENAME, d.DNAME FROM EMP e JOIN DEPT d ON d.DEPTNO = e.DEPTNO"
        ));
        // JOIN result sets are not editable even with qualified ROWID.
        let qualified_headers = vec!["E.ROWID".to_string(), "ENAME".to_string()];
        assert!(!ResultTableWidget::can_show_rowid_edit_actions(
            &qualified_headers,
            "SELECT e.ROWID, e.ENAME, d.DNAME FROM EMP e JOIN DEPT d ON d.DEPTNO = e.DEPTNO"
        ));
        let internal_alias_headers = vec!["SQ_INTERNAL_ROWID".to_string(), "ENAME".to_string()];
        assert!(!ResultTableWidget::can_show_rowid_edit_actions(
            &internal_alias_headers,
            "SELECT ENAME AS SQ_INTERNAL_ROWID, ENAME FROM EMP"
        ));
    }

    #[test]
    fn can_show_rowid_edit_actions_accepts_help_query_with_semicolon() {
        let headers = vec!["HELP.ROWID".to_string(), "TOPIC".to_string()];
        assert!(ResultTableWidget::can_show_rowid_edit_actions(
            &headers,
            "SELECT help.ROWID, help.* FROM help;"
        ));
    }

    #[test]
    fn detect_auto_hidden_rowid_col_hides_first_rowid_col_when_edit_mode_is_disabled() {
        let headers = vec!["EMP.ROWID".to_string(), "ENAME".to_string()];
        assert_eq!(
            ResultTableWidget::detect_auto_hidden_rowid_col(
                &headers,
                "SELECT ENAME FROM EMP",
                false,
            ),
            Some(0)
        );

        assert_eq!(
            ResultTableWidget::detect_auto_hidden_rowid_col(
                &headers,
                "SELECT ROWID, ENAME FROM EMP",
                false,
            ),
            Some(0)
        );
    }

    #[test]
    fn detect_auto_hidden_rowid_col_does_not_hide_when_rowid_is_not_first_col() {
        let headers = vec!["ENAME".to_string(), "EMP.ROWID".to_string()];
        assert_eq!(
            ResultTableWidget::detect_auto_hidden_rowid_col(
                &headers,
                "SELECT ENAME, ROWID FROM EMP",
                false,
            ),
            None
        );
    }

    #[test]
    fn detect_auto_hidden_rowid_col_does_not_hide_while_edit_mode_enabled() {
        let headers = vec!["EMP.ROWID".to_string(), "ENAME".to_string()];
        assert_eq!(
            ResultTableWidget::detect_auto_hidden_rowid_col(
                &headers,
                "SELECT ENAME FROM EMP",
                true
            ),
            None
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn display_result_keeps_staged_edits_when_save_request_fails() {
        let mut widget = ResultTableWidget::new();
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        *widget
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(ResultTableWidget::canonical_sql_signature(
                "UPDATE EMP SET ENAME = 'X' WHERE ROWID = 'AAABBB';",
            ));
        *widget
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some("SQ_SAVE_REQUEST:77".to_string());

        let failed = QueryResult {
            sql: "/* SQ_SAVE_REQUEST:77 */
UPDATE EMP SET ENAME = 'X' WHERE ROWID = 'AAABBB';"
                .to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            execution_time: std::time::Duration::from_millis(1),
            message: "ORA-00001".to_string(),
            is_select: false,
            success: false,
        };

        widget.display_result(&failed);

        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some());
        assert!(!*widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()));
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn display_result_restores_backup_when_matching_save_fails_without_live_session() {
        let mut widget = ResultTableWidget::new();

        let backup_session = TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: [(
                "AAABBB".to_string(),
                vec!["AAABBB".to_string(), "SCOTT".to_string()],
            )]
            .into_iter()
            .collect(),
            original_row_order: vec!["AAABBB".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "AAABBB".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        };
        widget.set_query_edit_backup(Some(QueryEditBackupState {
            headers: vec!["ROWID".to_string(), "ENAME".to_string()],
            full_data: vec![vec!["AAABBB".to_string(), "MILLER".to_string()]],
            source_sql: "SELECT ROWID, ENAME FROM EMP".to_string(),
            edit_session: backup_session,
            sort_state: None,
        }));

        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        *widget
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(ResultTableWidget::canonical_sql_signature(
                "UPDATE EMP SET ENAME = 'MILLER' WHERE ROWID = 'AAABBB';",
            ));
        *widget
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some("SQ_SAVE_REQUEST:99".to_string());

        let failed = QueryResult {
            sql: "/* SQ_SAVE_REQUEST:99 */
UPDATE EMP SET ENAME = 'MILLER' WHERE ROWID = 'AAABBB';"
                .to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            execution_time: std::time::Duration::from_millis(1),
            message: "ORA-00001".to_string(),
            is_select: false,
            success: false,
        };

        widget.display_result(&failed);

        assert!(!*widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()));
        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some());
        assert_eq!(
            widget
                .full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[vec!["AAABBB".to_string(), "MILLER".to_string()]]
        );
        assert!(widget
            .query_edit_backup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn display_result_keeps_grid_rows_after_matching_save_success() {
        let mut widget = ResultTableWidget::new();
        *widget
            .source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            "SELECT ROWID, ENAME FROM EMP".to_string();
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec!["ROWID".to_string(), "ENAME".to_string()];
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "SCOTT".to_string()]];
        widget.table.set_rows(1);
        widget.table.set_cols(2);

        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "AAABBB".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        });
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        *widget
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(ResultTableWidget::canonical_sql_signature(
                "UPDATE EMP SET ENAME = 'MILLER' WHERE ROWID = 'AAABBB';",
            ));
        *widget
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some("SQ_SAVE_REQUEST:42".to_string());

        let save_success = QueryResult {
            sql: "/* SQ_SAVE_REQUEST:42 */
UPDATE EMP SET ENAME = 'MILLER' WHERE ROWID = 'AAABBB';"
                .to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 1,
            execution_time: std::time::Duration::from_millis(1),
            message: "1 row updated".to_string(),
            is_select: false,
            success: true,
        };

        widget.display_result(&save_success);

        assert!(!*widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()));
        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
        assert_eq!(
            widget
                .full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[vec!["AAABBB".to_string(), "SCOTT".to_string()]]
        );
        assert_eq!(widget.table.rows(), 1);
        assert_eq!(widget.table.cols(), 2);
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn display_result_keeps_save_pending_for_non_matching_failure() {
        let mut widget = ResultTableWidget::new();
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        *widget
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(ResultTableWidget::canonical_sql_signature(
                "UPDATE EMP SET ENAME = 'MILLER' WHERE ROWID = 'AAABBB';",
            ));
        *widget
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some("SQ_SAVE_REQUEST:42".to_string());

        let unrelated_failure = QueryResult {
            sql: "SELECT * FROM BROKEN".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            execution_time: std::time::Duration::from_millis(1),
            message: "ORA-00942".to_string(),
            is_select: false,
            success: false,
        };

        widget.display_result(&unrelated_failure);

        assert!(*widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()));
        assert_eq!(
            widget
                .pending_save_request_tag
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_deref(),
            Some("SQ_SAVE_REQUEST:42")
        );
        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some());
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn display_result_clears_save_pending_for_terminal_failure_with_empty_sql() {
        let mut widget = ResultTableWidget::new();
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        *widget
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(ResultTableWidget::canonical_sql_signature(
                "UPDATE EMP SET ENAME = 'MILLER' WHERE ROWID = 'AAABBB';",
            ));
        *widget
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some("SQ_SAVE_REQUEST:42".to_string());

        let terminal_failure = QueryResult {
            sql: String::new(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            execution_time: std::time::Duration::from_millis(1),
            message: "Query cancelled".to_string(),
            is_select: false,
            success: false,
        };

        widget.display_result(&terminal_failure);

        assert!(!*widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()));
        assert!(widget
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
        assert!(widget
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some());
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn display_result_clears_save_pending_for_terminal_success_with_empty_sql() {
        let mut widget = ResultTableWidget::new();
        *widget
            .source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            "SELECT ROWID, ENAME FROM EMP".to_string();
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec!["ROWID".to_string(), "ENAME".to_string()];
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "MILLER".to_string()]];
        widget.table.set_rows(1);
        widget.table.set_cols(2);
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "AAABBB".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        });
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        *widget
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(ResultTableWidget::canonical_sql_signature(
                "UPDATE EMP SET ENAME = 'MILLER' WHERE ROWID = 'AAABBB';",
            ));
        *widget
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some("SQ_SAVE_REQUEST:501".to_string());

        let terminal_success = QueryResult {
            sql: String::new(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 1,
            execution_time: std::time::Duration::from_millis(1),
            message: "1 UPDATE row(s) affected".to_string(),
            is_select: false,
            success: true,
        };

        widget.display_result(&terminal_success);

        assert!(!*widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()));
        assert!(widget
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
        assert!(widget
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
        assert_eq!(
            widget
                .full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[vec!["AAABBB".to_string(), "MILLER".to_string()]]
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn display_result_clears_save_pending_when_request_tag_is_only_in_message() {
        let mut widget = ResultTableWidget::new();
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        *widget
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(ResultTableWidget::canonical_sql_signature(
                "UPDATE EMP SET ENAME = 'MILLER' WHERE ROWID = 'AAABBB';",
            ));
        *widget
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some("SQ_SAVE_REQUEST:101".to_string());

        let tagged_in_message = QueryResult {
            sql: String::new(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            execution_time: std::time::Duration::from_millis(1),
            message: "ORA-01013 user requested cancel /* SQ_SAVE_REQUEST:101 */".to_string(),
            is_select: false,
            success: false,
        };

        widget.display_result(&tagged_in_message);

        assert!(!*widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()));
        assert!(widget
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
        assert!(widget
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some());
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn display_result_clears_save_pending_when_statement_signature_matches() {
        let mut widget = ResultTableWidget::new();
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        *widget
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(ResultTableWidget::canonical_sql_signature(
                "BEGIN UPDATE EMP SET ENAME = 'MILLER' WHERE ROWID = 'AAABBB'; UPDATE EMP SET JOB = 'MANAGER' WHERE ROWID = 'AAABBB'; END;",
            ));
        *widget
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some("SQ_SAVE_REQUEST:313".to_string());
        *widget
            .pending_save_statement_signatures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = vec![
            ResultTableWidget::canonical_sql_signature(
                "UPDATE EMP SET ENAME = 'MILLER' WHERE ROWID = 'AAABBB';",
            ),
            ResultTableWidget::canonical_sql_signature(
                "UPDATE EMP SET JOB = 'MANAGER' WHERE ROWID = 'AAABBB';",
            ),
        ];

        let failed = QueryResult {
            sql: "UPDATE EMP SET ENAME = 'MILLER' WHERE ROWID = 'AAABBB'".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            execution_time: std::time::Duration::from_millis(1),
            message: "ORA-00001".to_string(),
            is_select: false,
            success: false,
        };

        widget.display_result(&failed);

        assert!(!*widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()));
        assert!(widget
            .pending_save_statement_signatures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn display_result_ignores_non_matching_result_while_save_is_pending() {
        let mut widget = ResultTableWidget::new();
        *widget
            .source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            "SELECT ROWID, ENAME FROM EMP".to_string();
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "SCOTT".to_string()]];
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "AAABBB".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        });
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        *widget
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(ResultTableWidget::canonical_sql_signature(
                "UPDATE EMP SET ENAME = 'MILLER' WHERE ROWID = 'AAABBB';",
            ));
        *widget
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some("SQ_SAVE_REQUEST:42".to_string());

        let unrelated = QueryResult {
            sql: "DELETE FROM EMP WHERE ROWID = 'ZZZ'".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            execution_time: std::time::Duration::from_millis(1),
            message: "1 row deleted".to_string(),
            is_select: false,
            success: true,
        };

        widget.display_result(&unrelated);

        assert!(*widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()));
        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some());
        assert_eq!(
            widget
                .full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[vec!["AAABBB".to_string(), "SCOTT".to_string()]]
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn display_result_keeps_staged_edits_when_non_save_query_fails() {
        let mut widget = ResultTableWidget::new();
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });
        *widget
            .source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            "SELECT ROWID, ENAME FROM EMP".to_string();
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "SCOTT".to_string()]];

        let failed = QueryResult {
            sql: "SELECT * FROM BROKEN".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            execution_time: std::time::Duration::from_millis(1),
            message: "ORA-00942".to_string(),
            is_select: false,
            success: false,
        };

        widget.display_result(&failed);

        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some());
        assert_eq!(
            widget
                .source_sql
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_str(),
            "SELECT ROWID, ENAME FROM EMP"
        );
        assert_eq!(
            widget
                .full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[vec!["AAABBB".to_string(), "SCOTT".to_string()]]
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn display_result_keeps_staged_edits_when_non_save_non_select_query_succeeds() {
        let mut widget = ResultTableWidget::new();
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec!["ROWID".to_string(), "ENAME".to_string()];
        *widget
            .source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            "SELECT ROWID, ENAME FROM EMP".to_string();
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "SCOTT".to_string()]];
        widget.table.set_rows(1);
        widget.table.set_cols(2);
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "AAABBB".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        });

        let commit_result = QueryResult {
            sql: "COMMIT".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            execution_time: std::time::Duration::from_millis(1),
            message: "Commit complete".to_string(),
            is_select: false,
            success: true,
        };

        widget.display_result(&commit_result);

        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some());
        assert_eq!(
            widget
                .source_sql
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_str(),
            "SELECT ROWID, ENAME FROM EMP"
        );
        assert_eq!(
            widget
                .full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[vec!["AAABBB".to_string(), "SCOTT".to_string()]]
        );
        assert_eq!(widget.table.rows(), 1);
        assert_eq!(widget.table.cols(), 2);
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn display_result_restores_staged_edits_after_select_failure_during_streaming() {
        let mut widget = ResultTableWidget::new();
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec!["ROWID".to_string(), "ENAME".to_string()];
        *widget
            .source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            "SELECT ROWID, ENAME FROM EMP".to_string();
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "MILLER".to_string()]];

        let mut original_rows_by_rowid = HashMap::new();
        original_rows_by_rowid.insert(
            "AAABBB".to_string(),
            vec!["AAABBB".to_string(), "SCOTT".to_string()],
        );
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid,
            original_row_order: vec!["AAABBB".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "AAABBB".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        });

        let new_headers = vec!["DEPTNO".to_string(), "DNAME".to_string()];
        widget.start_streaming(&new_headers);
        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());

        let failed = QueryResult {
            sql: "SELECT DEPTNO, DNAME FROM DEPT".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            execution_time: std::time::Duration::from_millis(1),
            message: "Query cancelled".to_string(),
            is_select: true,
            success: false,
        };
        widget.display_result(&failed);

        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some());
        assert_eq!(
            widget
                .source_sql
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_str(),
            "SELECT ROWID, ENAME FROM EMP"
        );
        assert_eq!(
            widget
                .headers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &["ROWID".to_string(), "ENAME".to_string()]
        );
        assert_eq!(
            widget
                .full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[vec!["AAABBB".to_string(), "MILLER".to_string()]]
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn commit_active_inline_edit_ignores_stale_editor_when_edit_mode_is_inactive() {
        let widget = ResultTableWidget::new();
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "SCOTT".to_string()]];

        let mut input = Input::default();
        input.set_value("MILLER");
        *widget
            .active_inline_edit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ActiveInlineEdit {
            row: 0,
            col: 1,
            input,
        });

        ResultTableWidget::commit_active_inline_edit_from_refs(
            &widget.table,
            &widget.full_data,
            &widget.edit_session,
            &widget.active_inline_edit,
        );

        assert_eq!(
            widget
                .full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[vec!["AAABBB".to_string(), "SCOTT".to_string()]]
        );
        assert!(widget
            .active_inline_edit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn clear_orphaned_query_edit_backup_recovers_select_start_interruption() {
        let mut widget = ResultTableWidget::new();
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec!["ROWID".to_string(), "ENAME".to_string()];
        *widget
            .source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            "SELECT ROWID, ENAME FROM EMP".to_string();
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "MILLER".to_string()]];
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: vec!["AAABBB".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "AAABBB".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        });

        let new_headers = vec!["DEPTNO".to_string(), "DNAME".to_string()];
        widget.start_streaming(&new_headers);
        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());

        assert!(widget.clear_orphaned_query_edit_backup());
        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some());
        assert!(!widget.clear_orphaned_query_edit_backup());
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn clear_orphaned_query_edit_backup_does_not_override_active_edit_session() {
        let mut widget = ResultTableWidget::new();
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec!["ROWID".to_string(), "ENAME".to_string()];
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["LIVE01".to_string(), "SCOTT".to_string()]];

        let active_session = TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: {
                let mut map = HashMap::new();
                map.insert(
                    "LIVE01".to_string(),
                    vec!["LIVE01".to_string(), "SMITH".to_string()],
                );
                map
            },
            original_row_order: vec!["LIVE01".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "LIVE01".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        };
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(active_session);

        widget.set_query_edit_backup(Some(QueryEditBackupState {
            headers: vec!["ROWID".to_string(), "ENAME".to_string()],
            full_data: vec![vec!["OLD01".to_string(), "MILLER".to_string()]],
            source_sql: "SELECT ROWID, ENAME FROM EMP".to_string(),
            edit_session: TableEditSession {
                rowid_col: 0,
                table_name: "EMP".to_string(),
                null_text: "NULL".to_string(),
                editable_columns: vec![(1, "ENAME".to_string())],
                original_rows_by_rowid: HashMap::new(),
                original_row_order: vec!["OLD01".to_string()],
                deleted_rowids: Vec::new(),
                row_states: vec![EditRowState::Existing {
                    rowid: "OLD01".to_string(),
                    explicit_null_cols: HashSet::new(),
                    dirty_cols: HashSet::new(),
                }],
            },
            sort_state: None,
        }));

        assert!(!widget.clear_orphaned_query_edit_backup());

        let current_rows = widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert_eq!(
            current_rows,
            vec![vec!["LIVE01".to_string(), "SCOTT".to_string()]]
        );
        assert!(widget
            .query_edit_backup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some());
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn clear_orphaned_save_request_recovers_interrupted_save_state() {
        let mut widget = ResultTableWidget::new();
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        *widget
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some("SQ_SAVE_REQUEST:9".to_string());

        assert!(widget.clear_orphaned_save_request());
        assert!(!*widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()));
        assert!(!mutex_load_bool(&widget.streaming_in_progress));
        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some());
        assert!(widget
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn save_edit_mode_executes_only_dml_without_replaying_source_select() {
        let mut widget = ResultTableWidget::new();
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec!["ROWID".to_string(), "ENAME".to_string()];
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "SCOTT".to_string()]];
        *widget
            .source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            "SELECT ROWID, ENAME FROM EMP; DELETE FROM AUDIT_LOG".to_string();

        let mut original_rows_by_rowid = HashMap::new();
        original_rows_by_rowid.insert(
            "AAABBB".to_string(),
            vec!["AAABBB".to_string(), "SMITH".to_string()],
        );
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid,
            original_row_order: vec!["AAABBB".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "AAABBB".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        });

        let captured_sql = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_sql_for_cb = captured_sql.clone();
        let callback: ResultGridSqlExecuteCallback =
            Arc::new(Mutex::new(Some(Box::new(move |sql| {
                captured_sql_for_cb
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(sql);
                Ok(())
            }))));
        *widget
            .execute_sql_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(callback);

        let save_result = widget.save_edit_mode();
        assert!(save_result.is_ok());

        let statements = captured_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert_eq!(statements.len(), 1);
        assert!(statements[0].contains("UPDATE EMP SET ENAME = 'SCOTT' WHERE ROWID = 'AAABBB';"));
        assert!(statements[0].contains("SQ_SAVE_REQUEST:"));
        assert!(!statements[0].contains("SELECT ROWID, ENAME FROM EMP"));
        assert!(!statements[0].contains("DELETE FROM AUDIT_LOG"));
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    #[cfg_attr(
        target_os = "linux",
        ignore = "save_edit_mode calls app::flush() which requires the FLTK UI thread"
    )]
    fn save_edit_mode_uses_explicit_null_literal_for_set_null_cells() {
        let mut widget = ResultTableWidget::new();
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec!["ROWID".to_string(), "ENAME".to_string()];
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "NULL".to_string()]];
        *widget
            .source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            "SELECT ROWID, ENAME FROM EMP".to_string();

        // Original value is "SMITH" (non-null) so that setting explicit null
        // produces a real UPDATE rather than being skipped as redundant.
        let mut original_rows_by_rowid = HashMap::new();
        original_rows_by_rowid.insert(
            "AAABBB".to_string(),
            vec!["AAABBB".to_string(), "SMITH".to_string()],
        );
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid,
            original_row_order: vec!["AAABBB".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "AAABBB".to_string(),
                explicit_null_cols: [1usize].into_iter().collect(),
                dirty_cols: HashSet::new(),
            }],
        });

        let captured_sql = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_sql_for_cb = captured_sql.clone();
        let callback: ResultGridSqlExecuteCallback =
            Arc::new(Mutex::new(Some(Box::new(move |sql| {
                captured_sql_for_cb
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(sql);
                Ok(())
            }))));
        *widget
            .execute_sql_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(callback);

        let save_result = widget.save_edit_mode();
        assert!(save_result.is_ok());

        let statements = captured_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert_eq!(statements.len(), 1);
        assert_eq!(
            statements[0],
            "UPDATE EMP SET ENAME = NULL WHERE ROWID = 'AAABBB';"
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    #[cfg_attr(
        target_os = "linux",
        ignore = "save_edit_mode calls app::flush() which requires the FLTK UI thread"
    )]
    fn save_edit_mode_skips_redundant_null_to_null_update() {
        let mut widget = ResultTableWidget::new();
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec!["ROWID".to_string(), "ENAME".to_string()];
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "NULL".to_string()]];
        *widget
            .source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            "SELECT ROWID, ENAME FROM EMP".to_string();

        // Original value is already NULL. Explicit null on the same cell
        // should be recognised as a no-op and skipped.
        let mut original_rows_by_rowid = HashMap::new();
        original_rows_by_rowid.insert(
            "AAABBB".to_string(),
            vec!["AAABBB".to_string(), "NULL".to_string()],
        );
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid,
            original_row_order: vec!["AAABBB".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "AAABBB".to_string(),
                explicit_null_cols: [1usize].into_iter().collect(),
                dirty_cols: HashSet::new(),
            }],
        });

        let captured_sql = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_sql_for_cb = captured_sql.clone();
        let callback: ResultGridSqlExecuteCallback =
            Arc::new(Mutex::new(Some(Box::new(move |sql| {
                captured_sql_for_cb
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(sql);
                Ok(())
            }))));
        *widget
            .execute_sql_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(callback);

        let save_result = widget.save_edit_mode();
        assert!(save_result.is_ok());

        // No SQL should have been executed because the change is redundant.
        let statements = captured_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert!(statements.is_empty());
    }

    #[test]
    fn try_execute_sql_returns_error_when_callback_is_missing() {
        let execute_sql_callback: Arc<Mutex<Option<ResultGridSqlExecuteCallback>>> =
            Arc::new(Mutex::new(None));
        let result = ResultTableWidget::try_execute_sql(
            &execute_sql_callback,
            "UPDATE EMP SET ENAME = 'A'".to_string(),
        );
        assert_eq!(result, Err("Edit callback is not connected.".to_string()));
    }

    #[test]
    fn try_execute_sql_invokes_registered_callback() {
        let captured_sql = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_sql_for_cb = captured_sql.clone();
        let callback: ResultGridSqlExecuteCallback =
            Arc::new(Mutex::new(Some(Box::new(move |sql| {
                captured_sql_for_cb
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(sql);
                Ok(())
            }))));
        let execute_sql_callback: Arc<Mutex<Option<ResultGridSqlExecuteCallback>>> =
            Arc::new(Mutex::new(Some(callback)));
        let sql = "DELETE FROM EMP WHERE ROWID = 'AAABBB'".to_string();

        let result = ResultTableWidget::try_execute_sql(&execute_sql_callback, sql.clone());
        assert!(result.is_ok());
        assert_eq!(
            captured_sql
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[sql]
        );
    }

    #[test]
    fn try_execute_sql_propagates_callback_error() {
        let callback: ResultGridSqlExecuteCallback = Arc::new(Mutex::new(Some(Box::new(|_sql| {
            Err("Another query is already running.".to_string())
        }))));
        let execute_sql_callback: Arc<Mutex<Option<ResultGridSqlExecuteCallback>>> =
            Arc::new(Mutex::new(Some(callback)));

        let result = ResultTableWidget::try_execute_sql(
            &execute_sql_callback,
            "UPDATE EMP SET ENAME='A'".to_string(),
        );
        assert_eq!(result, Err("Another query is already running.".to_string()));
    }

    #[test]
    fn try_execute_sql_restores_callback_for_consecutive_calls() {
        let call_count = Arc::new(Mutex::new(0usize));
        let call_count_for_cb = call_count.clone();
        let callback: ResultGridSqlExecuteCallback =
            Arc::new(Mutex::new(Some(Box::new(move |_sql| {
                let mut count = call_count_for_cb
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *count += 1;
                Ok(())
            }))));
        let execute_sql_callback: Arc<Mutex<Option<ResultGridSqlExecuteCallback>>> =
            Arc::new(Mutex::new(Some(callback.clone())));

        let first = ResultTableWidget::try_execute_sql(
            &execute_sql_callback,
            "UPDATE EMP SET ENAME='A'".to_string(),
        );
        let second = ResultTableWidget::try_execute_sql(
            &execute_sql_callback,
            "UPDATE EMP SET ENAME='B'".to_string(),
        );

        assert!(first.is_ok());
        assert!(second.is_ok());
        assert_eq!(
            *call_count
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            2
        );
        assert!(callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some());
    }

    #[test]
    fn try_execute_sql_recovers_when_callback_panics() {
        let call_count = Arc::new(Mutex::new(0usize));
        let call_count_for_cb = call_count.clone();
        let callback: ResultGridSqlExecuteCallback =
            Arc::new(Mutex::new(Some(Box::new(move |_sql| {
                let mut count = call_count_for_cb
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *count += 1;
                if *count == 1 {
                    panic!("simulate result-grid callback panic");
                }
                Ok(())
            }))));
        let execute_sql_callback: Arc<Mutex<Option<ResultGridSqlExecuteCallback>>> =
            Arc::new(Mutex::new(Some(callback.clone())));

        let first = ResultTableWidget::try_execute_sql(
            &execute_sql_callback,
            "UPDATE EMP SET ENAME='A'".to_string(),
        );
        assert_eq!(
            first,
            Err("Internal error: result-grid execute callback panicked.".to_string())
        );
        assert!(callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some());

        let second = ResultTableWidget::try_execute_sql(
            &execute_sql_callback,
            "UPDATE EMP SET ENAME='B'".to_string(),
        );
        assert!(second.is_ok());
        assert_eq!(
            *call_count
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            2
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn start_streaming_keeps_existing_rows_while_save_is_pending() {
        let mut widget = ResultTableWidget::new();
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "SCOTT".to_string()]];
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;

        let headers = vec!["ROWID".to_string(), "ENAME".to_string()];
        widget.start_streaming(&headers);

        assert!(!mutex_load_bool(&widget.streaming_in_progress));

        let rows = widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "AAABBB");
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn start_streaming_commits_active_inline_edit_while_save_is_pending() {
        let mut widget = ResultTableWidget::new();
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec!["ROWID".to_string(), "ENAME".to_string()];
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "SCOTT".to_string()]];
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "AAABBB".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        });
        let mut input = Input::default();
        input.set_value("MILLER");
        *widget
            .active_inline_edit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ActiveInlineEdit {
            row: 0,
            col: 1,
            input,
        });

        let headers = vec!["ROWID".to_string(), "ENAME".to_string()];
        widget.start_streaming(&headers);

        let rows = widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert_eq!(rows, vec![vec!["AAABBB".to_string(), "MILLER".to_string()]]);
        assert!(widget
            .active_inline_edit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn start_streaming_backup_captures_inline_edit_null_flags() {
        let mut widget = ResultTableWidget::new();
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec!["ROWID".to_string(), "ENAME".to_string()];
        *widget
            .source_sql
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            "SELECT ROWID, ENAME FROM EMP".to_string();
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "SCOTT".to_string()]];
        let mut original_rows_by_rowid = HashMap::new();
        original_rows_by_rowid.insert(
            "AAABBB".to_string(),
            vec!["AAABBB".to_string(), "SCOTT".to_string()],
        );
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid,
            original_row_order: vec!["AAABBB".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "AAABBB".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        });

        let mut input = Input::default();
        input.set_value("=NULL");
        *widget
            .active_inline_edit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ActiveInlineEdit {
            row: 0,
            col: 1,
            input,
        });

        let new_headers = vec!["DEPTNO".to_string(), "DNAME".to_string()];
        widget.start_streaming(&new_headers);

        let failed = QueryResult {
            sql: "SELECT DEPTNO, DNAME FROM DEPT".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            execution_time: std::time::Duration::from_millis(1),
            message: "Query cancelled".to_string(),
            is_select: true,
            success: false,
        };
        widget.display_result(&failed);

        let session = widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert!(
            session.is_some(),
            "edit session should be restored from backup"
        );
        let session = session.unwrap_or_else(|| TableEditSession {
            rowid_col: 0,
            table_name: String::new(),
            null_text: String::new(),
            editable_columns: Vec::new(),
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });
        let explicit_null = session
            .row_states
            .first()
            .map(ResultTableWidget::row_state_explicit_null_cols)
            .map(|cols| cols.contains(&1))
            .unwrap_or(false);
        assert!(explicit_null);
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn display_result_keeps_streamed_rows_when_select_is_cancelled() {
        let mut widget = ResultTableWidget::new();
        let headers = vec!["EMPNO".to_string(), "ENAME".to_string()];
        widget.start_streaming(&headers);
        widget.append_rows(vec![vec!["7369".to_string(), "SMITH".to_string()]]);
        widget.finish_streaming();

        let cancelled = QueryResult {
            sql: "SELECT EMPNO, ENAME FROM EMP".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            execution_time: std::time::Duration::from_millis(1),
            message: "Query cancelled".to_string(),
            is_select: true,
            success: false,
        };

        widget.display_result(&cancelled);

        assert_eq!(
            widget
                .headers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &["EMPNO".to_string(), "ENAME".to_string()]
        );
        assert_eq!(
            widget
                .full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[vec!["7369".to_string(), "SMITH".to_string()]]
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn streamed_rows_keep_font_height_without_post_render_metric_pass() {
        let mut widget = ResultTableWidget::new();
        let headers = vec!["EMPNO".to_string()];
        widget.start_streaming(&headers);
        widget.append_rows(vec![
            vec!["7369".to_string()],
            vec!["7499".to_string()],
            vec!["7521".to_string()],
        ]);
        widget.finish_streaming();

        let expected_height = ResultTableWidget::row_height_for_font(widget.font_settings.size());
        assert_eq!(widget.table.row_height(0), expected_height);
        assert_eq!(widget.table.row_height(2), expected_height);

        let result = QueryResult::new_select_streamed(
            "SELECT EMPNO FROM EMP",
            vec![crate::db::ColumnInfo {
                name: "EMPNO".to_string(),
                data_type: "NUMBER".to_string(),
            }],
            3,
            Duration::from_millis(1),
        );
        widget.display_result(&result);

        assert_eq!(widget.table.row_height(0), expected_height);
        assert_eq!(widget.table.row_height(2), expected_height);
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn append_rows_applies_widths_when_batch_reaches_sample_limit() {
        let mut widget = ResultTableWidget::new();
        let headers = vec!["A".to_string()];
        widget.start_streaming(&headers);
        let initial_width = widget.table.col_width(0);

        let mut rows = (0..WIDTH_SAMPLE_ROWS)
            .map(|_| vec!["1".to_string()])
            .collect::<Vec<_>>();
        rows[WIDTH_SAMPLE_ROWS - 1] = vec!["W".repeat(50)];

        widget.append_rows(rows);

        assert_eq!(
            mutex_load_usize(&widget.width_sampled_rows),
            WIDTH_SAMPLE_ROWS
        );
        assert!(
            widget.table.col_width(0) > initial_width,
            "sampled width should be applied before sampling is marked complete"
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn append_rows_applies_widths_for_first_small_lazy_batch() {
        let mut widget = ResultTableWidget::new();
        let headers = vec!["A".to_string()];
        widget.start_streaming(&headers);
        let initial_width = widget.table.col_width(0);

        let mut rows = (0..100).map(|_| vec!["1".to_string()]).collect::<Vec<_>>();
        rows[99] = vec!["W".repeat(50)];

        widget.append_rows(rows);

        assert_eq!(mutex_load_usize(&widget.width_sampled_rows), 100);
        assert!(
            widget.table.col_width(0) > initial_width,
            "first lazy batch should auto-fit sampled row contents"
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn font_change_updates_existing_and_new_row_heights() {
        let mut widget = ResultTableWidget::new();
        let headers = vec!["EMPNO".to_string()];
        widget.start_streaming(&headers);
        widget.append_rows(vec![vec!["7369".to_string()]]);

        let original_size = widget.font_settings.size();
        let original_height = ResultTableWidget::row_height_for_font(original_size);
        assert_eq!(widget.table.row_height(0), original_height);

        let updated_size = original_size.saturating_add(4);
        widget.apply_font_settings(widget.font_settings.profile(), updated_size);

        let updated_height = ResultTableWidget::row_height_for_font(updated_size);
        assert_eq!(widget.table.row_height(0), updated_height);

        widget.append_rows(vec![vec!["7499".to_string()], vec!["7521".to_string()]]);

        assert_eq!(widget.table.row_height(0), updated_height);
        assert_eq!(widget.table.row_height(1), updated_height);
        assert_eq!(widget.table.row_height(2), updated_height);
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn start_streaming_without_edit_session_clears_stale_backup_before_failure_result() {
        let mut widget = ResultTableWidget::new();
        widget.set_query_edit_backup(Some(QueryEditBackupState {
            headers: vec!["ROWID".to_string(), "ENAME".to_string()],
            full_data: vec![vec!["AAABBB".to_string(), "SCOTT".to_string()]],
            source_sql: "SELECT ROWID, ENAME FROM EMP".to_string(),
            edit_session: TableEditSession {
                rowid_col: 0,
                table_name: "EMP".to_string(),
                null_text: "NULL".to_string(),
                editable_columns: vec![(1, "ENAME".to_string())],
                original_rows_by_rowid: HashMap::new(),
                original_row_order: Vec::new(),
                deleted_rowids: Vec::new(),
                row_states: vec![EditRowState::Existing {
                    rowid: "AAABBB".to_string(),
                    explicit_null_cols: HashSet::new(),
                    dirty_cols: HashSet::new(),
                }],
            },
            sort_state: None,
        }));

        let headers = vec!["DEPTNO".to_string(), "DNAME".to_string()];
        widget.start_streaming(&headers);

        assert!(widget
            .query_edit_backup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());

        let failed = QueryResult {
            sql: "SELECT DEPTNO, DNAME FROM DEPT".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            execution_time: std::time::Duration::from_millis(1),
            message: "Query cancelled".to_string(),
            is_select: true,
            success: false,
        };
        widget.display_result(&failed);

        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
        assert_eq!(
            widget
                .headers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &["Result".to_string()]
        );
        assert_eq!(
            widget
                .full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[vec!["Query cancelled".to_string()]]
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn save_edit_mode_returns_error_when_save_is_already_pending() {
        let mut widget = ResultTableWidget::new();
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;

        let result = widget.save_edit_mode();
        assert_eq!(result, Err("Save is already in progress.".to_string()));
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn insert_and_delete_are_blocked_while_save_is_pending() {
        let mut widget = ResultTableWidget::new();
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec!["ROWID".to_string(), "ENAME".to_string()];
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;

        assert_eq!(
            widget.insert_row_in_edit_mode(),
            Err("Cannot insert rows while save is in progress.".to_string())
        );
        assert_eq!(
            widget.delete_selected_rows_in_edit_mode(),
            Err("Cannot delete rows while save is in progress.".to_string())
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn insert_and_delete_are_blocked_while_streaming_is_in_progress() {
        let mut widget = ResultTableWidget::new();
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec!["ROWID".to_string(), "ENAME".to_string()];
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });
        mutex_store_bool(&widget.streaming_in_progress, true);

        assert_eq!(
            widget.insert_row_in_edit_mode(),
            Err("Cannot insert rows while query rows are still loading.".to_string())
        );
        assert_eq!(
            widget.delete_selected_rows_in_edit_mode(),
            Err("Cannot delete rows while query rows are still loading.".to_string())
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn save_edit_mode_returns_error_while_streaming_is_in_progress() {
        let mut widget = ResultTableWidget::new();
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });
        mutex_store_bool(&widget.streaming_in_progress, true);

        let result = widget.save_edit_mode();
        assert_eq!(
            result,
            Err("Cannot save edits while query rows are still loading.".to_string())
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn delete_selected_rows_returns_error_when_selection_has_no_staged_rows() {
        let mut widget = ResultTableWidget::new();
        widget.table.set_rows(1);
        widget.table.set_cols(2);
        widget.table.set_selection(0, 0, 0, 0);
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });

        assert_eq!(
            widget.delete_selected_rows_in_edit_mode(),
            Err("No staged rows are available to delete.".to_string())
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn delete_selected_rows_returns_error_when_selection_is_outside_staged_rows() {
        let mut widget = ResultTableWidget::new();
        widget.table.set_rows(3);
        widget.table.set_cols(2);
        widget.table.set_selection(2, 0, 2, 0);
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "SCOTT".to_string()]];
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "AAABBB".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        });

        assert_eq!(
            widget.delete_selected_rows_in_edit_mode(),
            Err("Selected rows are outside the staged row range.".to_string())
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn set_null_commits_active_inline_edit_before_applying_selection() {
        let mut widget = ResultTableWidget::new();
        widget.table.set_rows(1);
        widget.table.set_cols(2);
        widget.table.set_selection(0, 1, 0, 1);
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec!["ROWID".to_string(), "ENAME".to_string()];
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "SCOTT".to_string()]];
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "AAABBB".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        });

        let mut input = Input::default();
        input.set_value("MILLER");
        *widget
            .active_inline_edit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ActiveInlineEdit {
            row: 0,
            col: 1,
            input,
        });

        let changed_result = ResultTableWidget::set_selected_cells_to_null_in_edit_mode(
            &widget.table,
            &widget.full_data,
            &widget.edit_session,
            &widget.pending_save_request,
            &widget.active_inline_edit,
        );
        assert!(changed_result.is_ok(), "set null should succeed");
        let changed = changed_result.unwrap_or(0);

        assert_eq!(changed, 1);
        assert_eq!(
            widget
                .full_data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
            vec![vec!["AAABBB".to_string(), "NULL".to_string()]]
        );
        assert!(widget
            .active_inline_edit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn set_null_returns_edit_mode_error_when_session_is_missing() {
        let mut widget = ResultTableWidget::new();
        widget.table.set_rows(1);
        widget.table.set_cols(2);
        widget.table.set_selection(0, 1, 0, 1);
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AAABBB".to_string(), "SCOTT".to_string()]];

        let result = ResultTableWidget::set_selected_cells_to_null_in_edit_mode(
            &widget.table,
            &widget.full_data,
            &widget.edit_session,
            &widget.pending_save_request,
            &widget.active_inline_edit,
        );

        assert_eq!(result, Err("Enable edit mode first.".to_string()));
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn set_null_returns_error_when_no_staged_rows_exist() {
        let mut widget = ResultTableWidget::new();
        widget.table.set_rows(1);
        widget.table.set_cols(2);
        widget.table.set_selection(0, 1, 0, 1);
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });

        let result = ResultTableWidget::set_selected_cells_to_null_in_edit_mode(
            &widget.table,
            &widget.full_data,
            &widget.edit_session,
            &widget.pending_save_request,
            &widget.active_inline_edit,
        );

        assert_eq!(
            result,
            Err("No staged rows are available for Set Null.".to_string())
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn cancel_edit_mode_returns_error_while_save_is_pending() {
        let mut widget = ResultTableWidget::new();
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;

        let result = widget.cancel_edit_mode();
        assert_eq!(
            result,
            Err("Cannot cancel edit mode while save is in progress.".to_string())
        );
        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some());
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn cancel_edit_mode_returns_error_while_streaming_is_in_progress() {
        let mut widget = ResultTableWidget::new();
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });
        mutex_store_bool(&widget.streaming_in_progress, true);

        let result = widget.cancel_edit_mode();
        assert_eq!(
            result,
            Err("Cannot cancel edit mode while query rows are still loading.".to_string())
        );
        assert!(widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some());
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn cancel_edit_mode_clears_stale_query_edit_backup() {
        let mut widget = ResultTableWidget::new();
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });
        widget.set_query_edit_backup(Some(QueryEditBackupState {
            headers: vec!["ROWID".to_string(), "ENAME".to_string()],
            full_data: vec![vec!["AAABBB".to_string(), "SCOTT".to_string()]],
            source_sql: "SELECT ROWID, ENAME FROM EMP".to_string(),
            edit_session: TableEditSession {
                rowid_col: 0,
                table_name: "EMP".to_string(),
                null_text: "NULL".to_string(),
                editable_columns: vec![(1, "ENAME".to_string())],
                original_rows_by_rowid: HashMap::new(),
                original_row_order: Vec::new(),
                deleted_rowids: Vec::new(),
                row_states: Vec::new(),
            },
            sort_state: None,
        }));

        let result = widget.cancel_edit_mode();
        assert!(result.is_ok());
        assert!(!widget.clear_orphaned_query_edit_backup());
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn cancel_edit_mode_error_without_session_keeps_existing_query_edit_backup() {
        let mut widget = ResultTableWidget::new();
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        *widget
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some("update emp".to_string());
        *widget
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some("SQ_SAVE_REQUEST:test".to_string());
        widget.set_query_edit_backup(Some(QueryEditBackupState {
            headers: vec!["ROWID".to_string(), "ENAME".to_string()],
            full_data: vec![vec!["AAABBB".to_string(), "SCOTT".to_string()]],
            source_sql: "SELECT ROWID, ENAME FROM EMP".to_string(),
            edit_session: TableEditSession {
                rowid_col: 0,
                table_name: "EMP".to_string(),
                null_text: "NULL".to_string(),
                editable_columns: vec![(1, "ENAME".to_string())],
                original_rows_by_rowid: HashMap::new(),
                original_row_order: Vec::new(),
                deleted_rowids: Vec::new(),
                row_states: Vec::new(),
            },
            sort_state: None,
        }));

        let result = widget.cancel_edit_mode();
        assert_eq!(result, Err("Edit mode is not active.".to_string()));
        assert!(widget.clear_orphaned_query_edit_backup());
        assert!(!*widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()));
        assert_eq!(
            widget
                .pending_save_sql_signature
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
            Some("update emp".to_string())
        );
        assert_eq!(
            widget
                .pending_save_request_tag
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
            Some("SQ_SAVE_REQUEST:test".to_string())
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn paste_returns_error_when_no_staged_rows_exist() {
        let mut widget = ResultTableWidget::new();
        widget.table.set_rows(1);
        widget.table.set_cols(2);
        widget.table.set_selection(0, 1, 0, 1);
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::new(),
            original_row_order: Vec::new(),
            deleted_rowids: Vec::new(),
            row_states: Vec::new(),
        });

        let result = ResultTableWidget::paste_clipboard_text_into_edit_mode(
            &widget.table,
            &widget.full_data,
            &widget.edit_session,
            &widget.pending_save_request,
            &widget.active_inline_edit,
            "A",
        );

        assert_eq!(
            result,
            Err("No staged rows are available for paste.".to_string())
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn clear_resets_pending_save_request_tag() {
        let mut widget = ResultTableWidget::new();
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        *widget
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some("update emp".to_string());
        *widget
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some("SQ_SAVE_REQUEST:stale".to_string());

        widget.clear();

        assert!(!*widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()));
        assert!(widget
            .pending_save_sql_signature
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
        assert!(widget
            .pending_save_request_tag
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
    }
}

impl Default for ResultTableWidget {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn matches_shortcut_key_accepts_current_ascii_key() {
        assert!(ResultTableWidget::matches_shortcut_key(
            Key::from_char('c'),
            Key::from_char('x'),
            'c',
        ));
    }

    #[test]
    fn matches_shortcut_key_accepts_original_ascii_key() {
        assert!(ResultTableWidget::matches_shortcut_key(
            Key::from_char('ㅊ'),
            Key::from_char('c'),
            'c',
        ));
    }

    #[test]
    fn ctrl_arrow_edge_matches_original_or_current_key() {
        assert_eq!(
            ResultTableWidget::edge_for_arrow_key(Key::Left, Key::from_char('x')),
            Some(ResultTableEdge::Left)
        );
        assert_eq!(
            ResultTableWidget::edge_for_arrow_key(Key::from_char('x'), Key::Down),
            Some(ResultTableEdge::Down)
        );
        assert_eq!(
            ResultTableWidget::edge_for_arrow_key(Key::PageDown, Key::PageDown),
            None
        );
    }

    #[test]
    fn home_end_map_to_horizontal_navigation_and_ctrl_to_vertical() {
        assert_eq!(
            ResultTableWidget::arrow_key_for_home_end(Key::Home, Key::from_char('x'), false),
            Some(Key::Left)
        );
        assert_eq!(
            ResultTableWidget::arrow_key_for_home_end(Key::from_char('x'), Key::End, false),
            Some(Key::Right)
        );
        assert_eq!(
            ResultTableWidget::arrow_key_for_home_end(Key::Home, Key::from_char('x'), true),
            Some(Key::Up)
        );
        assert_eq!(
            ResultTableWidget::arrow_key_for_home_end(Key::from_char('x'), Key::End, true),
            Some(Key::Down)
        );
        assert_eq!(
            ResultTableWidget::arrow_key_for_home_end(Key::Left, Key::Right, true),
            None
        );
    }

    #[test]
    fn context_menu_hit_accepts_column_header_and_empty_column_body() {
        assert!(ResultTableWidget::should_show_context_menu_for_hit(
            Some((0, 0)),
            None,
            None,
            false,
        ));
        assert!(ResultTableWidget::should_show_context_menu_for_hit(
            None,
            None,
            Some(0),
            false,
        ));
        assert!(ResultTableWidget::should_show_context_menu_for_hit(
            None, None, None, true,
        ));
        assert!(!ResultTableWidget::should_show_context_menu_for_hit(
            None, None, None, false,
        ));
    }

    #[test]
    fn lazy_fetch_more_navigation_keys_include_down_page_down_and_end() {
        assert!(ResultTableWidget::key_should_request_lazy_fetch_more(
            Key::Down
        ));
        assert!(ResultTableWidget::key_should_request_lazy_fetch_more(
            Key::PageDown
        ));
        assert!(ResultTableWidget::key_should_request_lazy_fetch_more(
            Key::End
        ));
        assert!(!ResultTableWidget::key_should_request_lazy_fetch_more(
            Key::Up
        ));
    }

    #[test]
    fn lazy_fetch_scroll_threshold_starts_at_full_scroll_range() {
        assert!(!ResultTableWidget::is_lazy_fetch_scroll_threshold_reached(
            79, 20, 100
        ));
        assert!(ResultTableWidget::is_lazy_fetch_scroll_threshold_reached(
            80, 20, 100
        ));
    }

    #[test]
    fn lazy_fetch_scroll_threshold_handles_fully_visible_rows() {
        assert!(ResultTableWidget::is_lazy_fetch_scroll_threshold_reached(
            0, 120, 100
        ));
        assert!(!ResultTableWidget::is_lazy_fetch_scroll_threshold_reached(
            0, 1, 0
        ));
    }

    #[test]
    fn lazy_fetch_more_for_session_sets_in_flight_and_suppresses_duplicate() {
        let session = Arc::new(Mutex::new(Some(7)));
        let in_flight = Arc::new(Mutex::new(HashSet::new()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_callback = requests.clone();
        let callback: LazyFetchCallback =
            Arc::new(Mutex::new(Some(Box::new(move |id, request| {
                requests_for_callback
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push((id, request));
                true
            }))));

        assert!(ResultTableWidget::request_lazy_fetch_more_for_session(
            7, &session, &callback, &in_flight
        ));
        assert!(!ResultTableWidget::request_lazy_fetch_more_for_session(
            7, &session, &callback, &in_flight
        ));

        assert_eq!(
            requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[(7, LazyFetchRequest::More)]
        );
        assert!(in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&7));
    }

    #[test]
    fn lazy_fetch_more_releases_in_flight_when_callback_rejects_request() {
        let session = Arc::new(Mutex::new(Some(7)));
        let in_flight = Arc::new(Mutex::new(HashSet::new()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_callback = requests.clone();
        let callback: LazyFetchCallback =
            Arc::new(Mutex::new(Some(Box::new(move |id, request| {
                requests_for_callback
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push((id, request));
                false
            }))));

        assert!(!ResultTableWidget::request_lazy_fetch_more_for_session(
            7, &session, &callback, &in_flight
        ));

        assert_eq!(
            requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[(7, LazyFetchRequest::More)]
        );
        assert!(in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
    }

    #[test]
    fn lazy_fetch_waiting_clears_stuck_in_flight_gate() {
        let session = Arc::new(Mutex::new(Some(9)));
        let in_flight = Arc::new(Mutex::new(HashSet::from([9])));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_callback = requests.clone();
        let callback: LazyFetchCallback =
            Arc::new(Mutex::new(Some(Box::new(move |id, request| {
                requests_for_callback
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push((id, request));
                true
            }))));

        // A fetch-more that produced no Rows event leaves the gate stuck.
        assert!(!ResultTableWidget::request_lazy_fetch_more_for_session(
            9, &session, &callback, &in_flight
        ));

        // The worker reporting waiting must release the gate.
        ResultTableWidget::clear_lazy_fetch_more_in_flight(&in_flight, 9);

        assert!(ResultTableWidget::request_lazy_fetch_more_for_session(
            9, &session, &callback, &in_flight
        ));
        assert_eq!(
            requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[(9, LazyFetchRequest::More)]
        );
    }

    #[test]
    fn lazy_fetch_more_for_session_ignores_stale_session() {
        let session = Arc::new(Mutex::new(Some(8)));
        let in_flight = Arc::new(Mutex::new(HashSet::new()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_callback = requests.clone();
        let callback: LazyFetchCallback =
            Arc::new(Mutex::new(Some(Box::new(move |id, request| {
                requests_for_callback
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push((id, request));
                true
            }))));

        assert!(!ResultTableWidget::request_lazy_fetch_more_for_session(
            7, &session, &callback, &in_flight
        ));

        assert!(requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
        assert!(in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn scrollbar_sequence_rearms_when_in_flight_suppresses_request() {
        let mut widget = ResultTableWidget::new();
        let headers = vec!["A".to_string()];
        widget.start_streaming(&headers);
        widget.append_rows(
            (0..30)
                .map(|value| vec![value.to_string()])
                .collect::<Vec<_>>(),
        );
        widget.table.set_row_position(widget.table.rows() - 2);

        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_callback = requests.clone();
        let callback: LazyFetchCallback =
            Arc::new(Mutex::new(Some(Box::new(move |id, request| {
                requests_for_callback
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push((id, request));
                true
            }))));
        widget.set_lazy_fetch_callback(callback);
        widget.set_lazy_fetch_session(77);
        widget
            .lazy_fetch_more_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(77);

        ResultTableWidget::request_lazy_fetch_more_for_scrollbar_sequence(
            &widget.table,
            &widget.lazy_fetch_session,
            &widget.lazy_fetch_callback,
            &widget.lazy_fetch_more_in_flight,
            &widget.drag_state,
        );

        assert!(requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
        assert!(
            !widget
                .drag_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .vertical_scrollbar_fetch_requested
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn append_rows_continues_lazy_fetch_when_appended_view_stays_near_bottom() {
        let mut widget = ResultTableWidget::new();
        let headers = vec!["A".to_string()];
        widget.start_streaming(&headers);
        widget.append_rows(
            (0..30)
                .map(|value| vec![value.to_string()])
                .collect::<Vec<_>>(),
        );
        widget.table.set_row_position(widget.table.rows() - 2);

        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_callback = requests.clone();
        let callback: LazyFetchCallback =
            Arc::new(Mutex::new(Some(Box::new(move |id, request| {
                requests_for_callback
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push((id, request));
                true
            }))));
        widget.set_lazy_fetch_callback(callback);
        widget.set_lazy_fetch_session(77);
        widget
            .lazy_fetch_more_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(77);

        widget.append_rows(vec![vec!["30".to_string()]]);

        assert_eq!(
            requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[(77, LazyFetchRequest::More)]
        );
        assert!(widget
            .lazy_fetch_more_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&77));
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn append_rows_stops_lazy_fetch_when_appended_view_drops_below_threshold() {
        let mut widget = ResultTableWidget::new();
        let headers = vec!["A".to_string()];
        widget.start_streaming(&headers);
        widget.append_rows(
            (0..30)
                .map(|value| vec![value.to_string()])
                .collect::<Vec<_>>(),
        );
        widget.table.set_row_position(widget.table.rows() - 2);

        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_callback = requests.clone();
        let callback: LazyFetchCallback =
            Arc::new(Mutex::new(Some(Box::new(move |id, request| {
                requests_for_callback
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push((id, request));
                true
            }))));
        widget.set_lazy_fetch_callback(callback);
        widget.set_lazy_fetch_session(77);
        widget
            .lazy_fetch_more_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(77);

        widget.append_rows(
            (30..1030)
                .map(|value| vec![value.to_string()])
                .collect::<Vec<_>>(),
        );

        assert!(requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
        assert!(!widget
            .lazy_fetch_more_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&77));
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn append_rows_below_threshold_rearms_scrollbar_sequence() {
        let mut widget = ResultTableWidget::new();
        let headers = vec!["A".to_string()];
        widget.start_streaming(&headers);
        widget.append_rows(
            (0..30)
                .map(|value| vec![value.to_string()])
                .collect::<Vec<_>>(),
        );
        widget.table.set_row_position(widget.table.rows() - 2);
        widget.set_lazy_fetch_session(77);
        widget
            .lazy_fetch_more_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(77);
        widget
            .drag_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .vertical_scrollbar_fetch_requested = true;

        widget.append_rows(
            (30..1030)
                .map(|value| vec![value.to_string()])
                .collect::<Vec<_>>(),
        );

        assert!(
            !widget
                .drag_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .vertical_scrollbar_fetch_requested
        );
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn lazy_fetch_finish_keeps_pending_export_until_close() {
        let mut widget = ResultTableWidget::new();
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = vec!["A".to_string()];
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = vec![vec!["1".to_string()]];

        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_callback = requests.clone();
        let callback: LazyFetchCallback =
            Arc::new(Mutex::new(Some(Box::new(move |session_id, request| {
                requests_for_callback
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push((session_id, request));
                true
            }))));
        widget.set_lazy_fetch_callback(callback);
        widget.set_lazy_fetch_session(77);

        let exported = Arc::new(Mutex::new(None));
        let exported_for_callback = exported.clone();
        assert!(widget
            .export_to_csv_after_fetch_all(Box::new(move |csv, row_count| {
                *exported_for_callback
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((csv, row_count));
            }))
            .is_none());

        widget.finish_streaming();
        assert!(exported
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());

        widget.clear_lazy_fetch_session(77, true);

        assert_eq!(
            requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[(77, LazyFetchRequest::All)]
        );
        assert_eq!(
            exported
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
                .map(|(_, row_count)| *row_count),
            Some(1)
        );
    }

    #[test]
    fn lazy_fetch_callback_is_invoked_without_holding_callback_lock() {
        let callback: LazyFetchCallback = Arc::new(Mutex::new(None));
        let callback_for_assert = callback.clone();
        *callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Box::new(move |session_id, request| {
                assert_eq!(session_id, 7);
                assert_eq!(request, LazyFetchRequest::More);
                assert!(callback_for_assert.try_lock().is_ok());
                true
            }));

        ResultTableWidget::invoke_lazy_fetch_callback(&callback, 7, LazyFetchRequest::More);
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn lazy_fetch_copy_does_not_request_fetch_all() {
        let mut widget = ResultTableWidget::new();
        *widget
            .headers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = vec!["A".to_string()];
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = vec![vec!["1".to_string()]];

        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_callback = requests.clone();
        let callback: LazyFetchCallback =
            Arc::new(Mutex::new(Some(Box::new(move |session_id, request| {
                requests_for_callback
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push((session_id, request));
                true
            }))));
        widget.set_lazy_fetch_callback(callback);
        widget.set_lazy_fetch_session(99);

        let _ = widget.copy();
        widget.copy_with_headers();

        assert!(requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
        assert!(widget
            .pending_lazy_actions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn lazy_fetch_abort_drops_pending_export_action() {
        let mut widget = ResultTableWidget::new();

        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_callback = requests.clone();
        let callback: LazyFetchCallback =
            Arc::new(Mutex::new(Some(Box::new(move |session_id, request| {
                requests_for_callback
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push((session_id, request));
                true
            }))));
        widget.set_lazy_fetch_callback(callback);
        widget.set_lazy_fetch_session(88);

        let exported = Arc::new(Mutex::new(false));
        let exported_for_callback = exported.clone();
        assert!(widget
            .export_to_csv_after_fetch_all(Box::new(move |_, _| {
                *exported_for_callback
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
            }))
            .is_none());

        widget.clear_lazy_fetch_state_for_abort();
        widget.clear_lazy_fetch_session(88, true);

        assert_eq!(
            requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[(88, LazyFetchRequest::All)]
        );
        assert!(!*exported
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()));
        assert!(widget
            .pending_lazy_actions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
    }

    #[test]
    fn escape_csv_field_quotes_carriage_return_values() {
        assert_eq!(
            ResultTableWidget::escape_csv_field("line1\rline2"),
            "\"line1\rline2\""
        );
    }

    #[test]
    fn csv_line_ending_matches_target_platform() {
        let expected = if cfg!(windows) { "\r\n" } else { "\n" };
        assert_eq!(ResultTableWidget::csv_line_ending(), expected);
    }

    #[test]
    fn sort_marker_for_column_reflects_sort_direction() {
        let asc = Some(ColumnSortState {
            col_idx: 1,
            direction: SortDirection::Ascending,
        });
        let desc = Some(ColumnSortState {
            col_idx: 1,
            direction: SortDirection::Descending,
        });
        assert_eq!(
            ResultTableWidget::sort_marker_for_column(asc, 1),
            Some(SORT_ASC_MARK)
        );
        assert_eq!(
            ResultTableWidget::sort_marker_for_column(desc, 1),
            Some(SORT_DESC_MARK)
        );
        assert_eq!(ResultTableWidget::sort_marker_for_column(desc, 0), None);
        assert_eq!(ResultTableWidget::sort_marker_for_column(None, 1), None);
    }

    #[test]
    fn next_sort_state_toggles_and_resets_for_new_column() {
        let first = ResultTableWidget::next_sort_state(None, 2);
        assert_eq!(
            first,
            ColumnSortState {
                col_idx: 2,
                direction: SortDirection::Ascending
            }
        );
        let second = ResultTableWidget::next_sort_state(Some(first), 2);
        assert_eq!(
            second,
            ColumnSortState {
                col_idx: 2,
                direction: SortDirection::Descending
            }
        );
        let third = ResultTableWidget::next_sort_state(Some(second), 0);
        assert_eq!(
            third,
            ColumnSortState {
                col_idx: 0,
                direction: SortDirection::Ascending
            }
        );
    }

    #[test]
    fn compare_row_values_for_sort_uses_numeric_order_for_numbers() {
        let left = vec!["2".to_string()];
        let right = vec!["10".to_string()];
        assert_eq!(
            ResultTableWidget::compare_row_values_for_sort(&left, &right, 0),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn compare_row_values_for_sort_places_numbers_before_text() {
        let number_row = vec!["42".to_string()];
        let text_row = vec!["ABC".to_string()];
        assert_eq!(
            ResultTableWidget::compare_row_values_for_sort(&number_row, &text_row, 0),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            ResultTableWidget::compare_row_values_for_sort(&text_row, &number_row, 0),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn sort_row_entries_reorders_rows_and_row_states_together() {
        let mut rows = vec![
            vec!["2".to_string(), "B".to_string()],
            vec!["1".to_string(), "A".to_string()],
            vec!["3".to_string(), "A".to_string()],
        ];
        let mut states = vec![
            EditRowState::Existing {
                rowid: "RID2".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            },
            EditRowState::Existing {
                rowid: "RID1".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            },
            EditRowState::Existing {
                rowid: "RID3".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            },
        ];

        assert!(ResultTableWidget::sort_row_entries(
            &mut rows,
            Some(&mut states),
            1,
            SortDirection::Ascending,
        ));

        assert_eq!(
            rows,
            vec![
                vec!["1".to_string(), "A".to_string()],
                vec!["3".to_string(), "A".to_string()],
                vec!["2".to_string(), "B".to_string()],
            ]
        );
        let rowids: Vec<String> = states
            .iter()
            .map(|state| match state {
                EditRowState::Existing { rowid, .. } => rowid.clone(),
                EditRowState::Inserted { .. } => "INSERTED".to_string(),
            })
            .collect();
        assert_eq!(
            rowids,
            vec!["RID1".to_string(), "RID3".to_string(), "RID2".to_string()]
        );
    }

    #[test]
    fn sort_row_entries_rejects_out_of_sync_row_states() {
        let mut rows = vec![vec!["2".to_string()], vec!["1".to_string()]];
        let mut states = vec![EditRowState::Existing {
            rowid: "RID2".to_string(),
            explicit_null_cols: HashSet::new(),
            dirty_cols: HashSet::new(),
        }];
        let original_rows = rows.clone();
        let original_states_len = states.len();

        assert!(!ResultTableWidget::sort_row_entries(
            &mut rows,
            Some(&mut states),
            0,
            SortDirection::Ascending,
        ));

        assert_eq!(rows, original_rows);
        assert_eq!(states.len(), original_states_len);
    }

    #[test]
    fn sort_row_entries_sorts_numeric_values_numerically() {
        let mut rows = vec![
            vec!["10".to_string()],
            vec!["2".to_string()],
            vec!["1".to_string()],
        ];
        assert!(ResultTableWidget::sort_row_entries(
            &mut rows,
            None,
            0,
            SortDirection::Ascending,
        ));
        assert_eq!(
            rows,
            vec![
                vec!["1".to_string()],
                vec!["2".to_string()],
                vec!["10".to_string()]
            ]
        );
    }

    #[test]
    fn pointer_moved_beyond_tolerance_uses_pixel_threshold() {
        assert!(!ResultTableWidget::pointer_moved_beyond_tolerance(
            100, 100, 103, 104, 4
        ));
        assert!(ResultTableWidget::pointer_moved_beyond_tolerance(
            100, 100, 106, 100, 4
        ));
        assert!(ResultTableWidget::pointer_moved_beyond_tolerance(
            100, 100, 100, 106, 4
        ));
    }

    #[test]
    fn canonical_sql_signature_normalizes_whitespace_and_trailing_semicolon() {
        let left = "  UPDATE   EMP SET ENAME = 'A'  WHERE ROWID = 'AAABBB'; ";
        let right = "UPDATE EMP\nSET ENAME = 'A' WHERE ROWID = 'AAABBB'";
        assert_eq!(
            ResultTableWidget::canonical_sql_signature(left),
            ResultTableWidget::canonical_sql_signature(right)
        );
    }

    #[test]
    fn canonical_sql_signature_ignores_comments_and_normalizes_keyword_case() {
        let left = "/* req */ update emp -- inline\nset ename = 'A' where rowid = 'AAABBB';";
        let right = "UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AAABBB'";
        assert_eq!(
            ResultTableWidget::canonical_sql_signature(left),
            ResultTableWidget::canonical_sql_signature(right)
        );
    }

    #[test]
    fn canonical_sql_signature_preserves_quoted_identifier_and_string_literal_case() {
        let left = "update \"CamelCase\".emp set ename = 'Mixed Case';";
        let right = "UPDATE \"CamelCase\".EMP SET ENAME = 'Mixed Case'";
        assert_eq!(
            ResultTableWidget::canonical_sql_signature(left),
            ResultTableWidget::canonical_sql_signature(right)
        );
    }

    #[test]
    fn matches_pending_save_signature_uses_normalized_sql() {
        let expected_signature =
            ResultTableWidget::canonical_sql_signature("UPDATE EMP SET ENAME = 'A';");
        assert!(ResultTableWidget::matches_pending_save_signature(
            Some(expected_signature.as_str()),
            " UPDATE   EMP\nSET ENAME = 'A'  ",
        ));
        assert!(!ResultTableWidget::matches_pending_save_signature(
            Some(expected_signature.as_str()),
            "DELETE FROM EMP",
        ));
    }
    #[test]
    fn matches_pending_save_tag_detects_embedded_request_marker() {
        assert!(ResultTableWidget::matches_pending_save_tag(
            Some("SQ_SAVE_REQUEST:7"),
            "/* SQ_SAVE_REQUEST:7 */ UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA';",
        ));
        assert!(!ResultTableWidget::matches_pending_save_tag(
            Some("SQ_SAVE_REQUEST:7"),
            "/* SQ_SAVE_REQUEST:9 */ UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA';",
        ));
    }

    #[test]
    fn matches_pending_save_tag_rejects_identifier_substring_match() {
        assert!(!ResultTableWidget::matches_pending_save_tag(
            Some("SQ_SAVE_REQUEST:7"),
            "/* SQ_SAVE_REQUEST:70 */ UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA';",
        ));
    }

    #[test]
    fn matches_pending_save_tag_in_message_rejects_identifier_substring_match() {
        assert!(!ResultTableWidget::matches_pending_save_tag_in_message(
            Some("SQ_SAVE_REQUEST:7"),
            "failed request SQ_SAVE_REQUEST:70 due to timeout",
        ));
        assert!(ResultTableWidget::matches_pending_save_tag_in_message(
            Some("SQ_SAVE_REQUEST:7"),
            "failed request SQ_SAVE_REQUEST:7 due to timeout",
        ));
    }

    #[test]
    fn matches_pending_save_matchers_require_registered_tracking_values() {
        let result_sql = "UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA'";
        assert!(!ResultTableWidget::matches_pending_save_signature(
            None, result_sql,
        ));
        assert!(!ResultTableWidget::matches_pending_save_tag(
            None, result_sql
        ));
    }

    #[test]
    fn save_signature_matches_dml_result_sql_without_request_comment() {
        let dml_script = "UPDATE EMP SET ENAME = 'SCOTT' WHERE ROWID = 'AAABBB';";
        let pending_signature = ResultTableWidget::canonical_sql_signature(dml_script);
        let result_sql = "UPDATE EMP SET ENAME = 'SCOTT' WHERE ROWID = 'AAABBB'";
        assert!(ResultTableWidget::matches_pending_save_signature(
            Some(pending_signature.as_str()),
            result_sql,
        ));
    }

    #[test]
    fn pending_save_terminal_matches_statement_signature_when_block_signature_differs() {
        let block_signature = ResultTableWidget::canonical_sql_signature(
            "BEGIN UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA'; UPDATE EMP SET JOB = 'B' WHERE ROWID = 'AA'; END;",
        );
        let statement_signatures = vec![ResultTableWidget::canonical_sql_signature(
            "UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA';",
        )];
        let result = QueryResult {
            success: false,
            message: "ORA-00001".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            is_select: false,
            sql: "UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA'".to_string(),
            execution_time: Duration::from_millis(0),
        };

        assert!(ResultTableWidget::is_pending_save_terminal_result(
            Some("SQ_SAVE_REQUEST:42"),
            Some(block_signature.as_str()),
            &statement_signatures,
            &result,
        ));
    }

    #[test]
    fn pending_save_terminal_does_not_match_statement_signature_for_select_packets() {
        let statement_signatures = vec![ResultTableWidget::canonical_sql_signature(
            "UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA';",
        )];
        let result = QueryResult {
            success: false,
            message: "unexpected".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            is_select: true,
            sql: "UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA'".to_string(),
            execution_time: Duration::from_millis(0),
        };

        assert!(!ResultTableWidget::is_pending_save_terminal_result(
            Some("SQ_SAVE_REQUEST:42"),
            Some("BEGIN ... END"),
            &statement_signatures,
            &result,
        ));
    }

    #[test]
    fn empty_sql_success_result_does_not_match_pending_save_fallback() {
        let result = QueryResult {
            success: true,
            message: "statement complete".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            is_select: false,
            sql: String::new(),
            execution_time: Duration::from_millis(0),
        };

        assert!(!ResultTableWidget::is_pending_save_terminal_result(
            Some("SQ_SAVE_REQUEST:11"),
            Some("UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA'"),
            &[],
            &result,
        ));
    }

    #[test]
    fn empty_sql_success_dml_message_matches_pending_save_fallback() {
        let result = QueryResult {
            success: true,
            message: "1 UPDATE row(s) affected".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            is_select: false,
            sql: String::new(),
            execution_time: Duration::from_millis(0),
        };

        assert!(ResultTableWidget::is_pending_save_terminal_result(
            Some("SQ_SAVE_REQUEST:11"),
            Some("UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA'"),
            &[],
            &result,
        ));
    }

    #[test]
    fn failed_cancel_message_with_empty_sql_matches_pending_save_fallback() {
        let result = QueryResult {
            success: false,
            message: "Query cancelled".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            is_select: false,
            sql: String::new(),
            execution_time: Duration::from_millis(0),
        };

        assert!(ResultTableWidget::is_pending_save_terminal_result(
            Some("SQ_SAVE_REQUEST:12"),
            Some("UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA'"),
            &[],
            &result,
        ));
    }

    #[test]
    fn pending_save_fallback_requires_tracking_metadata() {
        let result = QueryResult {
            success: false,
            message: "Query cancelled".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            is_select: false,
            sql: String::new(),
            execution_time: Duration::from_millis(0),
        };

        assert!(!ResultTableWidget::is_pending_save_terminal_result(
            None,
            None,
            &[],
            &result,
        ));
    }

    #[test]
    fn connection_loss_message_matches_not_connected_text() {
        assert!(ResultTableWidget::is_connection_loss_message(
            "Not connected to database",
        ));
    }

    #[test]
    fn connection_loss_message_matches_ora_01012_not_logged_on_text() {
        assert!(ResultTableWidget::is_connection_loss_message(
            "ORA-01012: not logged on",
        ));
    }

    #[test]
    fn connection_loss_message_matches_ora_03135_text() {
        assert!(ResultTableWidget::is_connection_loss_message(
            "ORA-03135: connection lost contact",
        ));
    }

    #[test]
    fn failed_connection_loss_with_empty_sql_matches_pending_save_fallback() {
        let result = QueryResult {
            success: false,
            message: "Connection was lost unexpectedly: ORA-03114".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            is_select: false,
            sql: String::new(),
            execution_time: Duration::from_millis(0),
        };

        assert!(ResultTableWidget::is_pending_save_terminal_result(
            Some("SQ_SAVE_REQUEST:12"),
            Some("UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA'"),
            &[],
            &result,
        ));
    }

    #[test]
    fn execution_abort_message_matches_ora_01013() {
        assert!(ResultTableWidget::is_execution_abort_message(
            "ORA-01013: user requested cancel of current operation",
        ));
    }

    #[test]
    fn execution_abort_message_matches_user_requested_cancel_text() {
        assert!(ResultTableWidget::is_execution_abort_message(
            "User requested cancel",
        ));
    }

    #[test]
    fn failed_cancel_with_non_empty_sql_does_not_match_pending_save_fallback() {
        let result = QueryResult {
            success: false,
            message: "Query cancelled".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            is_select: false,
            sql: "UPDATE DEPT SET DNAME = 'X' WHERE DEPTNO = 10".to_string(),
            execution_time: Duration::from_millis(0),
        };

        assert!(!ResultTableWidget::is_pending_save_terminal_result(
            Some("SQ_SAVE_REQUEST:12"),
            Some("UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA'"),
            &[],
            &result,
        ));
    }

    #[test]
    fn failed_non_abort_message_does_not_match_pending_save_fallback() {
        let result = QueryResult {
            success: false,
            message: "ORA-00942: table or view does not exist".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            is_select: false,
            sql: "SELECT * FROM EMP".to_string(),
            execution_time: Duration::from_millis(0),
        };

        assert!(!ResultTableWidget::is_pending_save_terminal_result(
            Some("SQ_SAVE_REQUEST:12"),
            Some("UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA'"),
            &[],
            &result,
        ));
    }

    #[test]
    fn failed_empty_sql_non_abort_message_does_not_match_pending_save_fallback() {
        let result = QueryResult {
            success: false,
            message: "ORA-00942: table or view does not exist".to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            is_select: false,
            sql: String::new(),
            execution_time: Duration::from_millis(0),
        };

        assert!(!ResultTableWidget::is_pending_save_terminal_result(
            Some("SQ_SAVE_REQUEST:12"),
            Some("UPDATE EMP SET ENAME = 'A' WHERE ROWID = 'AA'"),
            &[],
            &result,
        ));
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn clear_orphaned_save_request_also_clears_pending_stream_buffers() {
        let mut widget = ResultTableWidget::new();
        widget
            .pending_rows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(vec!["1".to_string()]);
        widget
            .pending_widths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(120);
        mutex_store_usize(&widget.width_sampled_rows, 7);
        *widget
            .pending_save_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;

        assert!(widget.clear_orphaned_save_request());
        assert!(widget
            .pending_rows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
        assert!(widget
            .pending_widths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
        assert_eq!(mutex_load_usize(&widget.width_sampled_rows), 0);
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn cancel_edit_mode_clears_pending_stream_buffers() {
        let mut widget = ResultTableWidget::new();
        *widget
            .edit_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TableEditSession {
            rowid_col: 0,
            table_name: "EMP".to_string(),
            null_text: "NULL".to_string(),
            editable_columns: vec![(1, "ENAME".to_string())],
            original_rows_by_rowid: HashMap::from([(
                "AA".to_string(),
                vec!["AA".to_string(), "SMITH".to_string()],
            )]),
            original_row_order: vec!["AA".to_string()],
            deleted_rowids: Vec::new(),
            row_states: vec![EditRowState::Existing {
                rowid: "AA".to_string(),
                explicit_null_cols: HashSet::new(),
                dirty_cols: HashSet::new(),
            }],
        });
        *widget
            .full_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            vec![vec!["AA".to_string(), "MILLER".to_string()]];
        widget
            .pending_rows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(vec!["stale".to_string()]);
        widget
            .pending_widths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(111);

        assert!(widget.cancel_edit_mode().is_ok());
        assert!(widget
            .pending_rows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
        assert!(widget
            .pending_widths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn display_result_select_clears_stale_pending_buffers() {
        let mut widget = ResultTableWidget::new();
        widget
            .pending_rows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(vec!["old".to_string()]);
        widget
            .pending_widths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(777);

        let result = QueryResult {
            sql: "SELECT ENAME FROM EMP".to_string(),
            columns: vec![crate::db::ColumnInfo {
                name: "ENAME".to_string(),
                data_type: "VARCHAR2".to_string(),
            }],
            rows: vec![vec!["SCOTT".to_string()]],
            row_count: 1,
            execution_time: Duration::from_millis(1),
            message: "ok".to_string(),
            is_select: true,
            success: true,
        };

        widget.display_result(&result);

        assert!(widget
            .pending_rows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty());
        let widths = widget
            .pending_widths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert!(!widths.is_empty());
        assert_ne!(widths, vec![777]);
    }
}
