use fltk::{
    app,
    button::Button,
    enums::{Align, CallbackTrigger, Event, FrameType, Key, Shortcut},
    frame::Frame,
    group::Group,
    input::Input,
    prelude::*,
};
use std::any::Any;
use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use crate::db::{DatabaseType, DbTableBrowsePagination, QueryExecutor};
use crate::ui::intellisense::{get_word_at_cursor, IntellisenseData, IntellisensePopup};
use crate::ui::result_tabs::ResultTabId;
use crate::ui::theme;
use crate::ui::ui_timeout;
use crate::utils::arithmetic::safe_div;

pub(crate) const TABLE_BROWSE_MATERIALIZE_MARKER: &str = "SQ_INTERNAL_TABLE_BROWSE";
pub(crate) const RESULT_FILTER_MARKER: &str = "SQ_INTERNAL_RESULT_FILTER";
pub(crate) const TABLE_BROWSE_PAGE_COLUMN: &str = "SQ_INTERNAL_PAGE_ROW";
pub(crate) const TABLE_BROWSE_DEFAULT_PAGE_SIZE: usize = 500;
pub(crate) const TABLE_BROWSE_FILTER_HEIGHT: i32 = 42;

/// The `ORDER BY` a header click leaves behind, toggling the clicked column
/// between ascending and descending.
///
/// The bar's text is the state: the user can edit it, and a redirected sort
/// runs on the server, so the grid's own sort state never advances. Anything
/// the click does not recognize as "just this column" — a different column, a
/// hand-written expression — is replaced by a plain ascending sort.
pub(crate) fn next_order_by_for_header_sort(current: &str, column: &str) -> String {
    let column = column.trim();
    let current = current.trim();
    let sorts_this_column_by = current
        .get(..column.len())
        .filter(|head| head.eq_ignore_ascii_case(column))
        .and_then(|_| current.get(column.len()..))
        .map(str::trim)
        .filter(|tail| {
            tail.is_empty() || tail.eq_ignore_ascii_case("ASC") || tail.eq_ignore_ascii_case("DESC")
        });
    match sorts_this_column_by {
        Some(tail) if tail.eq_ignore_ascii_case("DESC") => column.to_string(),
        Some(_) => format!("{column} DESC"),
        None => column.to_string(),
    }
}

/// Whether this statement is a table page this feature built, rather than
/// something the user ran. Such a statement bypasses lazy fetch, reserves
/// result-grid routing, and is never itself a relation to filter or re-page.
pub(crate) fn is_materialized_grid_statement(sql: &str) -> bool {
    carries_mark(sql, TABLE_BROWSE_MATERIALIZE_MARKER)
}

/// Whether `sql` carries `marker` WHERE [`marked_sql`] puts it.
///
/// Where, not whether. This asked `sql.contains(marker)`, which is a question
/// about the whole statement TEXT — and a statement's text includes its values.
/// A query selecting the marker as a string, or an import whose file held it in
/// a cell, was then read as a statement this app had written: its result got no
/// filter bar, the grids the user was looking at were not cleared for it, its
/// routing was reserved as an internal refill, and lazy fetch was turned off.
///
/// The mark is a COMMENT, and a value can never be one. Reading it back exactly
/// where the writer puts it is what makes it unforgeable — the two are an
/// inverse pair, and `a_value_cannot_forge_a_mark` holds them to it.
fn carries_mark(sql: &str, marker: &str) -> bool {
    let comment = format!("/* {marker} */");
    let trimmed = sql.trim_start();
    // `<KEYWORD> /* MARKER */ …`, which is where the writer puts it.
    let word = trimmed
        .find(|ch: char| ch.is_whitespace() || ch == '(')
        .unwrap_or(trimmed.len());
    if word > 0 && trimmed[word..].trim_start().starts_with(&comment) {
        return true;
    }
    // And the shape a statement with no leading word would get.
    trimmed.starts_with(&comment)
}

/// Whether this statement was written by this feature rather than by the user
/// — a table page, or a re-query of a result the user filtered.
///
/// Such a statement already contains someone's filter, so it must never become
/// the relation a new filter bar wraps: that would filter the filter, and every
/// apply would nest one level deeper. The mark travels in the statement text so
/// the answer holds however the result is routed.
pub(crate) fn is_generated_grid_statement(sql: &str) -> bool {
    is_materialized_grid_statement(sql) || carries_mark(sql, RESULT_FILTER_MARKER)
}

/// The same statement, marked as one this feature wrote for a filtered result.
pub(crate) fn marked_result_filter_sql(sql: &str) -> String {
    marked_sql(sql, RESULT_FILTER_MARKER)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TableBrowsePopupKeyAction {
    SelectPrev,
    SelectNext,
    SelectPrevPage,
    SelectNextPage,
    Confirm,
    Dismiss,
}

struct TableBrowsePopupShowReset<'a>(&'a AtomicBool);

impl Drop for TableBrowsePopupShowReset<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableBrowseTarget {
    pub db_type: DatabaseType,
    pub scope: Option<String>,
    pub table_name: String,
    pub relation_sql: String,
    pub completion_name: String,
    /// Whether the browsed relation may be edited in the grid. Views browse
    /// read-only: without a ROWID column the grid offers no edit actions.
    pub editable: bool,
    /// Column names to complete on when no metadata lookup can supply them.
    ///
    /// A browsed table resolves its columns through `completion_name`, but a
    /// filtered query result is a derived table — it has no name to look up, so
    /// the only place its columns exist is the result the grid is showing.
    /// Empty for a real relation, which keeps the metadata path untouched.
    pub result_columns: Vec<String>,
}

impl TableBrowseTarget {
    pub(crate) fn new(
        db_type: DatabaseType,
        scope: Option<String>,
        table_name: String,
        relation_sql: String,
        completion_name: String,
    ) -> Self {
        Self {
            db_type,
            scope,
            table_name,
            relation_sql,
            completion_name,
            editable: true,
            result_columns: Vec::new(),
        }
    }

    pub(crate) fn read_only(mut self) -> Self {
        self.editable = false;
        self
    }

    pub(crate) fn completion_tables(&self) -> Vec<String> {
        let mut tables = vec![self.completion_name.clone()];
        if !tables
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&self.table_name))
        {
            tables.push(self.table_name.clone());
        }
        tables
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TableBrowseClauses {
    pub(crate) where_expr: String,
    pub(crate) order_by_expr: String,
}

impl TableBrowseClauses {
    pub(crate) fn new(where_expr: String, order_by_expr: String) -> Self {
        Self {
            where_expr: where_expr.trim().to_string(),
            order_by_expr: order_by_expr.trim().to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TableBrowseNavigation {
    Page,
    Last,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TableBrowsePageRequest {
    pub(crate) result_tab_id: ResultTabId,
    pub(crate) target: TableBrowseTarget,
    pub(crate) clauses: TableBrowseClauses,
    pub(crate) offset: u64,
    pub(crate) page_size: usize,
    pub(crate) navigation: TableBrowseNavigation,
}

impl TableBrowsePageRequest {
    pub(crate) fn first(result_tab_id: ResultTabId, target: TableBrowseTarget) -> Self {
        Self {
            result_tab_id,
            target,
            clauses: TableBrowseClauses::default(),
            offset: 0,
            page_size: TABLE_BROWSE_DEFAULT_PAGE_SIZE,
            navigation: TableBrowseNavigation::Page,
        }
    }

    pub(crate) fn logical_sql(&self) -> Result<String, String> {
        build_logical_sql(&self.target, &self.clauses)
    }

    pub(crate) fn page_sql(&self) -> Result<String, String> {
        build_page_sql(self)
    }

    pub(crate) fn count_sql(&self) -> Result<String, String> {
        build_count_sql(&self.target, &self.clauses)
    }
}

pub(crate) type TableBrowseExecuteCallback =
    Arc<Mutex<Option<Box<dyn FnMut(TableBrowsePageRequest) -> Result<(), String>>>>>;

fn panic_payload_to_string(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

pub(crate) fn invoke_table_browse_execute_callback(
    callback: &TableBrowseExecuteCallback,
    request: TableBrowsePageRequest,
) -> Result<(), String> {
    let callback_fn = callback
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    let Some(mut callback_fn) = callback_fn else {
        return Err("Table browse callback is unavailable.".to_string());
    };

    let call_result = panic::catch_unwind(AssertUnwindSafe(|| callback_fn(request)));
    let mut callback_guard = callback
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if callback_guard.is_none() {
        *callback_guard = Some(callback_fn);
    }
    drop(callback_guard);

    match call_result {
        Ok(result) => result,
        Err(payload) => {
            let message = panic_payload_to_string(payload.as_ref());
            crate::utils::logging::log_error(
                "table_browse::callback",
                &format!("table browse callback panicked: {message}"),
            );
            eprintln!("table browse callback panicked: {message}");
            Err("Internal error: table browse callback panicked.".to_string())
        }
    }
}

/// The same statement, marked as a table page this feature materialized.
///
/// Public only so `verify_derived_table_wrap` can build the SQL it sends to a
/// real server with the SAME writer the app uses; it used to mirror this by
/// hand, which is a copy of a rule that has already drifted once. Not part of
/// the supported surface.
#[doc(hidden)]
pub fn marked_materialized_sql(sql: &str) -> String {
    marked_sql(sql, TABLE_BROWSE_MATERIALIZE_MARKER)
}

fn marked_sql(sql: &str, marker: &str) -> String {
    let trimmed_start = sql.len().saturating_sub(sql.trim_start().len());
    let trimmed = sql.trim_start();
    // After the leading KEYWORD, not in front of the statement. A comment that
    // opens a statement is one the executor's own splitter drops on the way to
    // execution — measured — so a mark written there was gone by the time
    // anything asked for it. Inside, it survives, and `WITH /* … */ x AS (…)`
    // is as valid as `SELECT /* … */ *`.
    let word = trimmed
        .find(|ch: char| ch.is_whitespace() || ch == '(')
        .unwrap_or(trimmed.len());
    if word == 0 {
        return format!("/* {marker} */\n{sql}");
    }
    format!(
        "{}{} /* {marker} */{}",
        &sql[..trimmed_start],
        &trimmed[..word],
        &trimmed[word..]
    )
}

/// Refuse a filter whose clauses would end the statement they are spliced
/// into. A filter runs one query or none.
fn validate_filter_clauses(
    clauses: &TableBrowseClauses,
    db_type: DatabaseType,
) -> Result<(), String> {
    for (label, clause) in [
        ("WHERE", &clauses.where_expr),
        ("ORDER BY", &clauses.order_by_expr),
    ] {
        if let Some(terminator) =
            crate::ui::result_filter::clause_statement_terminator(clause, db_type)
        {
            return Err(format!(
                "The {label} filter contains the statement terminator `{terminator}`. \
                 A filter is one expression, not a statement of its own."
            ));
        }
    }
    Ok(())
}

fn validate_single_statement(sql: &str, db_type: DatabaseType) -> Result<(), String> {
    let items =
        crate::ui::sql_editor::query_text::split_script_items_for_db_type(sql, Some(db_type));
    if items.len() == 1 && matches!(items.first(), Some(crate::db::ScriptItem::Statement(_))) {
        Ok(())
    } else {
        Err("WHERE and ORDER BY fields must contain one SQL expression, not multiple statements or tool commands.".to_string())
    }
}

pub(crate) fn build_logical_sql(
    target: &TableBrowseTarget,
    clauses: &TableBrowseClauses,
) -> Result<String, String> {
    if target.relation_sql.trim().is_empty() {
        return Err("The table name is empty.".to_string());
    }
    validate_filter_clauses(clauses, target.db_type)?;
    let mut sql = format!("SELECT * FROM {}", target.relation_sql);
    if !clauses.where_expr.is_empty() {
        sql.push_str("\nWHERE ");
        sql.push_str(&clauses.where_expr);
    }
    if !clauses.order_by_expr.is_empty() {
        sql.push_str("\nORDER BY ");
        sql.push_str(&clauses.order_by_expr);
    }
    validate_single_statement(&sql, target.db_type)?;
    Ok(sql)
}

pub(crate) fn build_count_sql(
    target: &TableBrowseTarget,
    clauses: &TableBrowseClauses,
) -> Result<String, String> {
    validate_filter_clauses(clauses, target.db_type)?;
    let mut sql = format!(
        "SELECT COUNT(*) AS SQ_TOTAL_ROWS FROM {}",
        target.relation_sql
    );
    if !clauses.where_expr.is_empty() {
        sql.push_str("\nWHERE ");
        sql.push_str(&clauses.where_expr);
    }
    validate_single_statement(&sql, target.db_type)?;
    Ok(marked_materialized_sql(&sql))
}

pub(crate) fn build_page_sql(request: &TableBrowsePageRequest) -> Result<String, String> {
    if request.page_size == 0 {
        return Err("Page size must be greater than zero.".to_string());
    }
    let page_size =
        u64::try_from(request.page_size).map_err(|_| "Page size is too large.".to_string())?;
    let fetch_size = page_size
        .checked_add(1)
        .ok_or_else(|| "Page range is too large.".to_string())?;
    let upper_bound = request
        .offset
        .checked_add(fetch_size)
        .ok_or_else(|| "Page range is too large.".to_string())?;
    let logical_sql = request.logical_sql()?;

    match request.target.db_type.table_browse_spec().pagination {
        DbTableBrowsePagination::Rownum => {
            let rowid_sql = if request.target.editable {
                let injected = QueryExecutor::maybe_inject_rowid_for_editing(&logical_sql);
                QueryExecutor::rowid_safe_execution_sql(&logical_sql, &injected)
            } else {
                logical_sql.clone()
            };
            let sql = format!(
                "SELECT *\nFROM (\n  SELECT sq_page_source.*, ROWNUM AS {TABLE_BROWSE_PAGE_COLUMN}\n  FROM (\n{inner}\n  ) sq_page_source\n  WHERE ROWNUM <= {upper_bound}\n)\nWHERE {TABLE_BROWSE_PAGE_COLUMN} > {offset}",
                inner = rowid_sql
                    .lines()
                    .map(|line| format!("    {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                offset = request.offset,
            );
            Ok(marked_materialized_sql(&sql))
        }
        DbTableBrowsePagination::LimitOffset => Ok(format!(
            "{}\nLIMIT {} OFFSET {}",
            marked_materialized_sql(&logical_sql),
            fetch_size,
            request.offset
        )),
    }
}

pub(crate) fn last_page_offset(total_rows: u64, page_size: usize) -> u64 {
    let Ok(page_size) = u64::try_from(page_size) else {
        return 0;
    };
    if total_rows == 0 || page_size == 0 {
        0
    } else {
        safe_div(total_rows - 1, page_size) * page_size
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TableBrowseFilterFocus {
    Where,
    Order,
}

#[derive(Clone)]
pub(crate) struct TableBrowseFilterBar {
    group: Group,
    where_input: Input,
    order_input: Input,
    where_clear: Button,
    order_clear: Button,
    where_label: Frame,
    order_label: Frame,
    where_popup: Arc<Mutex<IntellisensePopup>>,
    order_popup: Arc<Mutex<IntellisensePopup>>,
    // Shared so the clone that deactivates the bar and the clone that
    // reactivates it agree on which input to hand focus back to, and on the
    // widget FLTK bounced focus to in the meantime.
    focus_restore: Arc<Mutex<Option<(TableBrowseFilterFocus, Option<usize>)>>>,
}

impl TableBrowseFilterBar {
    pub(crate) fn new(
        x: i32,
        y: i32,
        w: i32,
        target: TableBrowseTarget,
        intellisense_data: Arc<Mutex<IntellisenseData>>,
        result_tab_id: ResultTabId,
        callback: TableBrowseExecuteCallback,
    ) -> Self {
        let mut group = Group::new(x, y, w.max(1), TABLE_BROWSE_FILTER_HEIGHT, None);
        group.set_frame(FrameType::FlatBox);
        group.set_color(theme::panel_bg());
        group.begin();

        let mut where_label = Frame::default().with_label("WHERE");
        where_label.set_align(Align::Right | Align::Inside);
        where_label.set_label_color(theme::text_secondary());
        let mut where_input = Input::default();
        Self::style_input(&mut where_input, "Condition only; press Enter to apply");
        let mut where_clear = Button::default().with_label("×");
        Self::style_clear_button(&mut where_clear, "Clear WHERE and reload the first page");

        let mut order_label = Frame::default().with_label("ORDER BY");
        order_label.set_align(Align::Right | Align::Inside);
        order_label.set_label_color(theme::text_secondary());
        let mut order_input = Input::default();
        Self::style_input(
            &mut order_input,
            "Sort expressions only; press Enter to apply",
        );
        let mut order_clear = Button::default().with_label("×");
        Self::style_clear_button(&mut order_clear, "Clear ORDER BY and reload the first page");

        group.end();

        let where_popup = Arc::new(Mutex::new(IntellisensePopup::new()));
        let order_popup = Arc::new(Mutex::new(IntellisensePopup::new()));
        let where_popup_showing = Arc::new(AtomicBool::new(false));
        let order_popup_showing = Arc::new(AtomicBool::new(false));
        Self::install_popup_selection(&where_input, &where_popup);
        Self::install_popup_selection(&order_input, &order_popup);

        Self::install_input_handler(
            &mut where_input,
            order_input.clone(),
            where_popup.clone(),
            order_popup.clone(),
            target.clone(),
            intellisense_data.clone(),
            result_tab_id,
            callback.clone(),
            true,
            where_popup_showing,
        );
        Self::install_input_handler(
            &mut order_input,
            where_input.clone(),
            order_popup.clone(),
            where_popup.clone(),
            target.clone(),
            intellisense_data,
            result_tab_id,
            callback.clone(),
            false,
            order_popup_showing,
        );

        {
            let mut where_input_for_clear = where_input.clone();
            let order_input_for_clear = order_input.clone();
            let target_for_clear = target.clone();
            let callback_for_clear = callback.clone();
            where_clear.set_callback(move |_| {
                where_input_for_clear.set_value("");
                Self::invoke_execute(
                    &callback_for_clear,
                    Self::first_page_request(
                        result_tab_id,
                        target_for_clear.clone(),
                        &where_input_for_clear,
                        &order_input_for_clear,
                    ),
                );
            });
        }
        {
            let where_input_for_clear = where_input.clone();
            let mut order_input_for_clear = order_input.clone();
            let target_for_clear = target;
            order_clear.set_callback(move |_| {
                order_input_for_clear.set_value("");
                Self::invoke_execute(
                    &callback,
                    Self::first_page_request(
                        result_tab_id,
                        target_for_clear.clone(),
                        &where_input_for_clear,
                        &order_input_for_clear,
                    ),
                );
            });
        }

        let mut bar = Self {
            group,
            where_input,
            order_input,
            where_clear,
            order_clear,
            where_label,
            order_label,
            where_popup,
            order_popup,
            focus_restore: Arc::new(Mutex::new(None)),
        };
        bar.layout(x, y, w);
        bar
    }

    fn style_input(input: &mut Input, tooltip: &str) {
        input.set_color(theme::input_bg());
        input.set_text_color(theme::text_primary());
        input.set_selection_color(theme::selection_soft());
        input.set_frame(FrameType::RFlatBox);
        input.set_trigger(CallbackTrigger::Changed);
        input.set_tooltip(tooltip);
        theme::apply_text_input_inset(input);
    }

    fn style_clear_button(button: &mut Button, tooltip: &str) {
        button.set_color(theme::button_subtle());
        button.set_label_color(theme::text_secondary());
        button.set_frame(FrameType::RFlatBox);
        button.set_tooltip(tooltip);
        theme::install_button_hover(button);
    }

    fn install_popup_selection(input: &Input, popup: &Arc<Mutex<IntellisensePopup>>) {
        let mut input = input.clone();
        popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set_selected_callback(move |selected| {
                Self::replace_current_word(&mut input, &selected);
            });
    }

    #[allow(clippy::too_many_arguments)]
    fn install_input_handler(
        input: &mut Input,
        other_input: Input,
        popup: Arc<Mutex<IntellisensePopup>>,
        other_popup: Arc<Mutex<IntellisensePopup>>,
        target: TableBrowseTarget,
        intellisense_data: Arc<Mutex<IntellisenseData>>,
        result_tab_id: ResultTabId,
        callback: TableBrowseExecuteCallback,
        is_where_input: bool,
        popup_showing: Arc<AtomicBool>,
    ) {
        // No Event::Push or Event::Focus arm: Fl_Input already takes focus on
        // FL_PUSH and accepts it on FL_FOCUS, and its native handler runs first
        // here. Calling take_focus() from a FL_FOCUS handler recurses without
        // bound, because Fl_Widget::take_focus() dispatches FL_FOCUS before it
        // sets Fl::focus(), leaving has_focus() false inside the handler.
        input.handle(move |input, event| match event {
            Event::KeyDown => {
                let key =
                    Self::shortcut_key_for_layout(app::event_key(), app::event_original_key());
                let shortcut = app::event_state();
                let ctrl_or_cmd =
                    shortcut.contains(Shortcut::Ctrl) || shortcut.contains(Shortcut::Command);
                let input_has_focus = input.has_focus();
                let popup_visible = popup
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .is_visible();

                if ctrl_or_cmd && key == Key::from_char(' ') && input_has_focus {
                    Self::show_suggestions(
                        input,
                        &popup,
                        &intellisense_data,
                        &target,
                        &popup_showing,
                        true,
                    );
                    return true;
                }
                if Self::should_hide_popup_on_modifier_keydown(popup_visible, key) {
                    popup
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .hide();
                    return false;
                }
                if popup_visible {
                    match Self::popup_key_action(key) {
                        Some(
                            TableBrowsePopupKeyAction::SelectPrev
                            | TableBrowsePopupKeyAction::SelectNext
                            | TableBrowsePopupKeyAction::SelectPrevPage
                            | TableBrowsePopupKeyAction::SelectNextPage
                            | TableBrowsePopupKeyAction::Dismiss,
                        ) if !input_has_focus => {
                            return false;
                        }
                        Some(TableBrowsePopupKeyAction::SelectPrev) => {
                            popup
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .select_prev();
                            return true;
                        }
                        Some(TableBrowsePopupKeyAction::SelectNext) => {
                            popup
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .select_next();
                            return true;
                        }
                        Some(TableBrowsePopupKeyAction::SelectPrevPage) => {
                            popup
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .select_prev_page();
                            return true;
                        }
                        Some(TableBrowsePopupKeyAction::SelectNextPage) => {
                            popup
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .select_next_page();
                            return true;
                        }
                        Some(TableBrowsePopupKeyAction::Confirm) => {
                            if !input_has_focus {
                                return false;
                            }
                            let selected = {
                                let mut popup = popup
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                let selected = popup.get_selected();
                                popup.hide();
                                selected
                            };
                            if let Some(selected) = selected {
                                Self::replace_current_word(input, &selected);
                                Self::retain_input_focus(input);
                                return true;
                            }
                        }
                        Some(TableBrowsePopupKeyAction::Dismiss) => {
                            popup
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .hide();
                            return true;
                        }
                        None => {}
                    }
                }
                if matches!(key, Key::Enter | Key::KPEnter) {
                    if !input_has_focus {
                        return false;
                    }
                    if Self::enter_commits_ime_composition(app::compose_state()) {
                        return true;
                    }
                    other_popup
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .hide();
                    let (where_input, order_input) = if is_where_input {
                        (input.clone(), other_input.clone())
                    } else {
                        (other_input.clone(), input.clone())
                    };
                    Self::invoke_execute(
                        &callback,
                        Self::first_page_request(
                            result_tab_id,
                            target.clone(),
                            &where_input,
                            &order_input,
                        ),
                    );
                    return true;
                }
                if key == Key::Escape {
                    return false;
                }
                false
            }
            Event::Shortcut => {
                if !input.has_focus() {
                    return false;
                }
                let key =
                    Self::shortcut_key_for_layout(app::event_key(), app::event_original_key());
                let popup_visible = popup
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .is_visible();
                Self::should_consume_popup_shortcut(popup_visible, key)
            }
            Event::KeyUp => {
                let key =
                    Self::shortcut_key_for_layout(app::event_key(), app::event_original_key());
                let shortcut = app::event_state();
                let ctrl_or_cmd =
                    shortcut.contains(Shortcut::Ctrl) || shortcut.contains(Shortcut::Command);
                if ctrl_or_cmd && key == Key::from_char(' ') {
                    return true;
                }
                if matches!(key, Key::Left | Key::Right) {
                    popup
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .position_below_input_caret(input);
                } else if !matches!(
                    key,
                    Key::Up
                        | Key::Down
                        | Key::PageUp
                        | Key::PageDown
                        | Key::Enter
                        | Key::KPEnter
                        | Key::Escape
                        | Key::Tab
                ) && !Self::is_modifier_key(key)
                {
                    Self::show_suggestions(
                        input,
                        &popup,
                        &intellisense_data,
                        &target,
                        &popup_showing,
                        false,
                    );
                }
                false
            }
            Event::Paste => {
                let mut input = input.clone();
                let popup = popup.clone();
                let intellisense_data = intellisense_data.clone();
                let target = target.clone();
                let popup_showing = popup_showing.clone();
                crate::ui::ui_timeout::schedule(0.0, move || {
                    Self::show_suggestions(
                        &mut input,
                        &popup,
                        &intellisense_data,
                        &target,
                        &popup_showing,
                        false,
                    );
                });
                false
            }
            Event::Released => {
                let input = input.clone();
                let popup = popup.clone();
                crate::ui::ui_timeout::schedule(0.0, move || {
                    if input.was_deleted() {
                        return;
                    }
                    popup
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .position_below_input_caret(&input);
                });
                false
            }
            Event::Unfocus => {
                if popup_showing.load(Ordering::Acquire) {
                    return false;
                }
                let unfocus_x = app::event_x_root();
                let unfocus_y = app::event_y_root();
                // Showing the top-level popup can synchronously dispatch Unfocus
                // while show_suggestions still holds this mutex. Never block on
                // that reentrant event or the UI thread will deadlock itself.
                match popup.try_lock() {
                    Ok(mut popup) => {
                        let popup_visible = popup.is_visible();
                        let pointer_inside_popup =
                            popup_visible && popup.contains_point(unfocus_x, unfocus_y);
                        if Self::should_hide_popup_on_unfocus(popup_visible, pointer_inside_popup) {
                            popup.hide();
                        }
                    }
                    Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                        let mut popup = poisoned.into_inner();
                        let popup_visible = popup.is_visible();
                        let pointer_inside_popup =
                            popup_visible && popup.contains_point(unfocus_x, unfocus_y);
                        if Self::should_hide_popup_on_unfocus(popup_visible, pointer_inside_popup) {
                            popup.hide();
                        }
                    }
                    Err(std::sync::TryLockError::WouldBlock) => {}
                }
                false
            }
            _ => false,
        });
    }

    fn shortcut_key_for_layout(key: Key, original_key: Key) -> Key {
        if (0..=0x7f).contains(&key.bits()) {
            key
        } else {
            original_key
        }
    }

    /// On macOS the Enter that commits an IME composition arrives as an
    /// ordinary Enter while `Fl::compose_state` is still set. Running the
    /// filter on it would fire the query mid-word, so let the composition
    /// finish and apply the filter on the next Enter. Shares the editor's
    /// rule so both typing paths agree on what an IME Enter is.
    fn enter_commits_ime_composition(compose_state: i32) -> bool {
        cfg!(target_os = "macos")
            && crate::ui::SqlEditorWidget::should_handle_enter_during_ime_composition(compose_state)
    }

    fn popup_key_action(key: Key) -> Option<TableBrowsePopupKeyAction> {
        match key {
            Key::Up => Some(TableBrowsePopupKeyAction::SelectPrev),
            Key::Down => Some(TableBrowsePopupKeyAction::SelectNext),
            Key::PageUp => Some(TableBrowsePopupKeyAction::SelectPrevPage),
            Key::PageDown => Some(TableBrowsePopupKeyAction::SelectNextPage),
            Key::Enter | Key::KPEnter | Key::Tab => Some(TableBrowsePopupKeyAction::Confirm),
            Key::Escape => Some(TableBrowsePopupKeyAction::Dismiss),
            _ => None,
        }
    }

    fn should_consume_popup_shortcut(popup_visible: bool, key: Key) -> bool {
        popup_visible
            && matches!(
                Self::popup_key_action(key),
                Some(
                    TableBrowsePopupKeyAction::SelectPrev
                        | TableBrowsePopupKeyAction::SelectNext
                        | TableBrowsePopupKeyAction::SelectPrevPage
                        | TableBrowsePopupKeyAction::SelectNextPage
                        | TableBrowsePopupKeyAction::Confirm
                )
            )
    }

    fn is_modifier_key(key: Key) -> bool {
        matches!(
            key,
            Key::ShiftL
                | Key::ShiftR
                | Key::ControlL
                | Key::ControlR
                | Key::AltL
                | Key::AltR
                | Key::MetaL
                | Key::MetaR
                | Key::CapsLock
        )
    }

    fn should_hide_popup_on_modifier_keydown(popup_visible: bool, key: Key) -> bool {
        popup_visible
            && matches!(
                key,
                Key::ShiftL
                    | Key::ShiftR
                    | Key::ControlL
                    | Key::ControlR
                    | Key::AltL
                    | Key::AltR
                    | Key::MetaL
                    | Key::MetaR
            )
    }

    fn retain_input_focus(input: &Input) {
        // Fl_Widget::take_focus() dispatches FL_FOCUS to the widget before it
        // sets Fl::focus(), so any handler that calls back here while the event
        // is in flight would recurse until the stack overflows. Refuse to
        // re-enter rather than trusting every future handler to be careful.
        thread_local! {
            static TAKING_FOCUS: Cell<bool> = const { Cell::new(false) };
        }
        if input.was_deleted() || TAKING_FOCUS.with(Cell::get) {
            return;
        }

        let mut immediate_focus = input.clone();
        TAKING_FOCUS.with(|taking| taking.set(true));
        let _ = immediate_focus.take_focus();
        TAKING_FOCUS.with(|taking| taking.set(false));

        let mut delayed_focus = input.clone();
        ui_timeout::schedule(0.0, move || {
            if delayed_focus.was_deleted() {
                return;
            }
            let _ = delayed_focus.take_focus();
        });
    }

    fn should_hide_popup_on_unfocus(popup_visible: bool, pointer_inside_popup: bool) -> bool {
        popup_visible && !pointer_inside_popup
    }

    fn completion_is_suppressed_at_cursor(
        value: &str,
        cursor: usize,
        db_type: DatabaseType,
    ) -> bool {
        let mut cursor = cursor.min(value.len());
        while cursor > 0 && !value.is_char_boundary(cursor) {
            cursor -= 1;
        }
        let (_, lex_mode) = crate::sql_parser_engine::lexical_spans_with_initial_mode(
            value.get(..cursor).unwrap_or(""),
            db_type.is_mysql_or_mariadb(),
            crate::sql_parser_engine::LexMode::Idle,
        );
        matches!(
            lex_mode,
            crate::sql_parser_engine::LexMode::SingleQuote
                | crate::sql_parser_engine::LexMode::LineComment
                | crate::sql_parser_engine::LexMode::BlockComment
                | crate::sql_parser_engine::LexMode::QQuote { .. }
                | crate::sql_parser_engine::LexMode::DollarQuote { .. }
                | crate::sql_parser_engine::LexMode::ForeignModuleSource
                | crate::sql_parser_engine::LexMode::ForeignInlineSource { .. }
        )
    }

    fn should_open_completion(prefix: &str, force: bool) -> bool {
        force || !prefix.is_empty()
    }

    /// What the filter popup shows: a derived relation's own columns first,
    /// then everything the metadata engine offered.
    ///
    /// For a browsed table `result_columns` is empty and this is exactly the
    /// engine's own answer, unchanged. For a filtered query result the engine
    /// can contribute keywords but no columns — there is no table name for it
    /// to resolve — so the result's headers have to lead.
    fn merge_filter_suggestions(
        result_columns: &[String],
        prefix: &str,
        from_metadata: Vec<String>,
    ) -> Vec<String> {
        let mut suggestions = Self::result_column_suggestions(result_columns, prefix);
        let mut seen: std::collections::HashSet<String> =
            suggestions.iter().map(|s| s.to_uppercase()).collect();
        for suggestion in from_metadata {
            if seen.insert(suggestion.to_uppercase()) {
                suggestions.push(suggestion);
            }
        }
        suggestions.truncate(crate::ui::intellisense::MAX_SUGGESTIONS);
        suggestions
    }

    /// Prefix-match a result's own column names, keeping their reported
    /// spelling and their result order.
    ///
    /// Blank names are skipped: `SET HEADING OFF` blanks them on the way to the
    /// grid, and a blank cannot be typed into a filter anyway.
    fn result_column_suggestions(columns: &[String], prefix: &str) -> Vec<String> {
        if columns.is_empty() {
            return Vec::new();
        }
        let prefix_upper = prefix.trim().to_uppercase();
        let mut seen = std::collections::HashSet::new();
        columns
            .iter()
            .map(|column| column.trim())
            .filter(|column| !column.is_empty())
            .filter(|column| column.to_uppercase().starts_with(&prefix_upper))
            .filter(|column| seen.insert(column.to_uppercase()))
            .map(str::to_string)
            .collect()
    }

    fn first_page_request(
        result_tab_id: ResultTabId,
        target: TableBrowseTarget,
        where_input: &Input,
        order_input: &Input,
    ) -> TableBrowsePageRequest {
        TableBrowsePageRequest {
            result_tab_id,
            target,
            clauses: TableBrowseClauses::new(where_input.value(), order_input.value()),
            offset: 0,
            page_size: 0,
            navigation: TableBrowseNavigation::Page,
        }
    }

    fn invoke_execute(callback: &TableBrowseExecuteCallback, request: TableBrowsePageRequest) {
        if let Err(message) = invoke_table_browse_execute_callback(callback, request) {
            crate::ui::alert_on_main(&message);
        }
    }

    fn show_suggestions(
        input: &mut Input,
        popup: &Arc<Mutex<IntellisensePopup>>,
        intellisense_data: &Arc<Mutex<IntellisenseData>>,
        target: &TableBrowseTarget,
        popup_showing: &AtomicBool,
        force: bool,
    ) {
        if input.was_deleted() || !input.has_focus() {
            popup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .hide();
            return;
        }
        let value = input.value();
        let cursor = usize::try_from(input.position())
            .unwrap_or_default()
            .min(value.len());
        if Self::completion_is_suppressed_at_cursor(&value, cursor, target.db_type) {
            popup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .hide();
            return;
        }
        let (prefix, _, _) = get_word_at_cursor(&value, cursor);
        if !Self::should_open_completion(&prefix, force) {
            popup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .hide();
            return;
        }
        let tables = target.completion_tables();
        let from_metadata = intellisense_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get_suggestions_for_db(
                &prefix,
                true,
                Some(&tables),
                false,
                true,
                Some(target.db_type),
            );
        let suggestions =
            Self::merge_filter_suggestions(&target.result_columns, &prefix, from_metadata);
        let mut popup = popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if suggestions.is_empty() {
            popup.hide();
        } else {
            popup_showing.store(true, Ordering::Release);
            let _show_reset = TableBrowsePopupShowReset(popup_showing);
            popup.show_suggestions_below_input_caret(suggestions, input);
            drop(popup);
            let _ = input.take_focus();
        }
    }

    pub(crate) fn capture_tour_show_where_popup(
        &mut self,
    ) -> (Input, Arc<Mutex<IntellisensePopup>>) {
        self.where_input.set_value("DEPTNO = 20 AND E");
        self.order_input.set_value("EMPNO ASC");
        let _ = self
            .where_input
            .set_position(i32::try_from(self.where_input.value().len()).unwrap_or(i32::MAX));
        self.where_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .show_suggestions_below_input_caret(
                vec![
                    "EMPNO".to_string(),
                    "ENAME".to_string(),
                    "JOB".to_string(),
                    "DEPTNO".to_string(),
                    "SAL".to_string(),
                ],
                &self.where_input,
            );
        (self.where_input.clone(), self.where_popup.clone())
    }

    pub(crate) fn capture_tour_show_order_popup(
        &mut self,
    ) -> (Input, Arc<Mutex<IntellisensePopup>>) {
        self.where_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .hide();
        let _ = self
            .order_input
            .set_position(i32::try_from(self.order_input.value().len()).unwrap_or(i32::MAX));
        self.order_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .show_suggestions_below_input_caret(
                vec![
                    "EMPNO".to_string(),
                    "ENAME".to_string(),
                    "JOB".to_string(),
                    "DEPTNO".to_string(),
                    "SAL".to_string(),
                ],
                &self.order_input,
            );
        (self.order_input.clone(), self.order_popup.clone())
    }

    fn replace_current_word(input: &mut Input, selected: &str) {
        if input.was_deleted() {
            return;
        }
        let value = input.value();
        let cursor = usize::try_from(input.position())
            .unwrap_or_default()
            .min(value.len());
        let (_, start, end) = get_word_at_cursor(&value, cursor);
        let mut next = String::with_capacity(
            value
                .len()
                .saturating_sub(end.saturating_sub(start))
                .saturating_add(selected.len()),
        );
        next.push_str(value.get(..start).unwrap_or(""));
        next.push_str(selected);
        next.push_str(value.get(end..).unwrap_or(""));
        input.set_value(&next);
        let mut caret = start.saturating_add(selected.len());
        if selected.ends_with("()") {
            caret = caret.saturating_sub(1);
        }
        let _ = input.set_position(i32::try_from(caret).unwrap_or(i32::MAX));
        input.redraw();
    }

    pub(crate) fn layout(&mut self, x: i32, y: i32, w: i32) {
        const HORIZONTAL_PADDING: i32 = 8;
        const VERTICAL_PADDING: i32 = 8;
        const GAP: i32 = 6;
        const WHERE_LABEL_WIDTH: i32 = 50;
        const ORDER_LABEL_WIDTH: i32 = 72;
        const CLEAR_WIDTH: i32 = 24;

        self.group
            .resize(x, y, w.max(1), TABLE_BROWSE_FILTER_HEIGHT);
        let control_y = y + VERTICAL_PADDING;
        let control_h = TABLE_BROWSE_FILTER_HEIGHT - VERTICAL_PADDING * 2;
        let fixed = HORIZONTAL_PADDING * 2
            + WHERE_LABEL_WIDTH
            + ORDER_LABEL_WIDTH
            + CLEAR_WIDTH * 2
            + GAP * 5;
        let editor_width = safe_div((w - fixed).max(80), 2);
        let mut cursor_x = x + HORIZONTAL_PADDING;
        self.where_label
            .resize(cursor_x, control_y, WHERE_LABEL_WIDTH, control_h);
        cursor_x += WHERE_LABEL_WIDTH + GAP;
        self.where_input
            .resize(cursor_x, control_y, editor_width, control_h);
        cursor_x += editor_width + GAP;
        self.where_clear
            .resize(cursor_x, control_y, CLEAR_WIDTH, control_h);
        cursor_x += CLEAR_WIDTH + GAP;
        self.order_label
            .resize(cursor_x, control_y, ORDER_LABEL_WIDTH, control_h);
        cursor_x += ORDER_LABEL_WIDTH + GAP;
        let remaining = (x + w - HORIZONTAL_PADDING - CLEAR_WIDTH - GAP - cursor_x).max(40);
        self.order_input
            .resize(cursor_x, control_y, remaining, control_h);
        cursor_x += remaining + GAP;
        self.order_clear
            .resize(cursor_x, control_y, CLEAR_WIDTH, control_h);
        self.group.redraw();
    }

    pub(crate) fn set_active(&mut self, active: bool) {
        if active {
            self.where_input.activate();
            self.order_input.activate();
            self.where_clear.activate();
            self.order_clear.activate();
            self.restore_thrown_focus();
        } else {
            // Fl_Widget::deactivate() throws focus, and FLTK then restores the
            // enclosing group's savedfocus_ (typically the object browser tree
            // the table was opened from). Remember the focused input so it can
            // be handed focus back once the inputs accept focus again.
            let focused = if self.where_input.has_focus() {
                Some(TableBrowseFilterFocus::Where)
            } else if self.order_input.has_focus() {
                Some(TableBrowseFilterFocus::Order)
            } else {
                None
            };
            self.hide_popups();
            self.where_input.deactivate();
            self.order_input.deactivate();
            self.where_clear.deactivate();
            self.order_clear.deactivate();
            *self
                .focus_restore
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                focused.map(|target| (target, Self::current_focus_id()));
        }
    }

    /// Hands focus back to the input that lost it to `deactivate()`, but only
    /// while focus still sits where FLTK bounced it. If the user moved focus
    /// somewhere else while the page was loading, that choice wins.
    fn restore_thrown_focus(&mut self) {
        let Some((target, bounced_focus)) = self
            .focus_restore
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        else {
            return;
        };
        if Self::current_focus_id() != bounced_focus {
            return;
        }
        match target {
            TableBrowseFilterFocus::Where => Self::retain_input_focus(&self.where_input),
            TableBrowseFilterFocus::Order => Self::retain_input_focus(&self.order_input),
        }
    }

    /// Identity of the focused widget as a plain integer, so the filter bar
    /// stays `Send` (a raw `Fl_Widget` pointer would not be).
    fn current_focus_id() -> Option<usize> {
        app::focus().map(|widget| widget.as_widget_ptr() as usize)
    }

    /// Say which of the result's columns this backend cannot resolve in a
    /// filter, so the user reads it before typing one rather than as a driver
    /// error afterwards.
    pub(crate) fn note_ambiguous_columns(&mut self, columns: &[String]) {
        if columns.is_empty() {
            return;
        }
        let note = format!(
            "\n\nThis result repeats the column name {}, so naming one of them here fails. Every \
             other column filters normally.",
            columns.join(", ")
        );
        for input in [&mut self.where_input, &mut self.order_input] {
            let tooltip = format!("{}{note}", input.tooltip().unwrap_or_default());
            input.set_tooltip(&tooltip);
        }
    }

    /// The current WHERE text, so a caller rebuilding the request keeps it.
    pub(crate) fn where_text(&self) -> String {
        self.where_input.value()
    }

    /// The current ORDER BY text, which is also the sort state a header click
    /// advances.
    pub(crate) fn order_by_text(&self) -> String {
        self.order_input.value()
    }

    /// Show an ORDER BY the user did not type — a redirected header-sort click.
    pub(crate) fn set_order_by_text(&mut self, expr: &str) {
        self.order_input.set_value(expr);
    }

    pub(crate) fn focus_where_input(&self) {
        Self::retain_input_focus(&self.where_input);
    }

    pub(crate) fn hide_popups(&self) {
        self.where_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .hide();
        self.order_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .hide();
    }

    pub(crate) fn cleanup_for_close(&mut self) {
        self.where_input.handle(|_, _| false);
        self.order_input.handle(|_, _| false);
        self.where_clear.set_callback(|_| {});
        self.order_clear.set_callback(|_| {});

        self.where_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .delete_for_close();
        self.order_popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .delete_for_close();
    }
}

#[cfg(test)]
mod tests {
    /// The mark survives the road between being written and being read.
    ///
    /// The reader is positional now, so anything that reshapes the statement
    /// between the two would turn a page of this feature's own into a user
    /// query — no reservation, the grids cleared under it, lazy fetch back on.
    /// The executor's own splitter is that road, so it is the one asked.
    #[test]
    fn a_mark_survives_the_splitter_that_carries_it() {
        use crate::db::query::{QueryExecutor, ScriptItem};

        for sql in [
            "SELECT * FROM (SELECT a FROM t) WHERE ROWNUM <= 500",
            "  select a from t",
            "WITH x AS (SELECT 1 FROM DUAL) SELECT * FROM x",
            "SELECT * FROM t;",
        ] {
            for marker in [
                super::TABLE_BROWSE_MATERIALIZE_MARKER,
                super::RESULT_FILTER_MARKER,
            ] {
                let marked = super::marked_sql(sql, marker);
                assert!(
                    super::carries_mark(&marked, marker),
                    "the writer and the reader disagree about {marked}"
                );
                let items = QueryExecutor::split_script_items(&marked);
                assert_eq!(items.len(), 1, "{marked} split into {} items", items.len());
                let ScriptItem::Statement(statement) = &items[0] else {
                    panic!("{marked} did not come back as a statement");
                };
                assert!(
                    super::carries_mark(statement, marker),
                    "the splitter lost the mark: {statement}"
                );
            }
        }
    }

    /// A value cannot forge a mark this app writes for itself.
    ///
    /// The mark is a COMMENT at the head of the statement. Reading it with
    /// `contains` asked about the whole TEXT — values included — so a query
    /// that merely SELECTED the marker, or an import whose file held it in a
    /// cell, was read as one of this feature's own statements: its result got
    /// no filter bar, the grids on screen were not cleared for it, its routing
    /// was reserved as an internal refill, and lazy fetch was turned off.
    #[test]
    fn a_value_cannot_forge_a_mark() {
        // The writer's two shapes both read back.
        let page = super::marked_sql("SELECT * FROM EMP", super::TABLE_BROWSE_MATERIALIZE_MARKER);
        assert!(super::is_materialized_grid_statement(&page), "{page}");
        assert!(super::is_generated_grid_statement(&page));
        let non_select = super::marked_sql(
            "WITH t AS (SELECT 1) SELECT * FROM t",
            super::TABLE_BROWSE_MATERIALIZE_MARKER,
        );
        assert!(
            super::is_materialized_grid_statement(&non_select),
            "{non_select}"
        );
        let filtered = super::marked_result_filter_sql("SELECT * FROM (SELECT 1) WHERE 1 = 1");
        assert!(super::is_generated_grid_statement(&filtered), "{filtered}");
        assert!(!super::is_materialized_grid_statement(&filtered));

        // And a value cannot.
        for forged in [
            "SELECT 'SQ_INTERNAL_TABLE_BROWSE' FROM DUAL",
            "SELECT 'SQ_INTERNAL_RESULT_FILTER' FROM DUAL",
            "INSERT INTO NOTES (BODY) VALUES ('/* SQ_INTERNAL_TABLE_BROWSE */')",
            "SELECT * FROM EMP WHERE NAME = '/* SQ_INTERNAL_RESULT_FILTER */'",
            "SELECT * FROM SQ_INTERNAL_TABLE_BROWSE",
        ] {
            assert!(
                !super::is_generated_grid_statement(forged),
                "a value forged the mark: {forged}"
            );
        }

        // An ordinary user query is not marked either.
        assert!(!super::is_generated_grid_statement("SELECT * FROM EMP"));
        assert!(!super::is_generated_grid_statement("  select 1 from dual"));
    }

    use super::*;

    #[test]
    fn plain_enter_applies_the_filter() {
        assert!(!TableBrowseFilterBar::enter_commits_ime_composition(0));
    }

    fn result_columns() -> Vec<String> {
        ["EMPNO", "ENAME", "ENAME_UPPER", "DEPTNO"]
            .iter()
            .map(|name| (*name).to_string())
            .collect()
    }

    #[test]
    fn a_filtered_result_completes_on_its_own_columns() {
        // Regression: a derived relation has no name to look up, so the filter
        // bar offered no columns at all until the target carried them.
        assert_eq!(
            TableBrowseFilterBar::result_column_suggestions(&result_columns(), "EN"),
            vec!["ENAME".to_string(), "ENAME_UPPER".to_string()]
        );
    }

    #[test]
    fn result_column_matching_ignores_case_but_keeps_the_reported_spelling() {
        let columns = vec!["HireDate".to_string()];
        assert_eq!(
            TableBrowseFilterBar::result_column_suggestions(&columns, "hire"),
            vec!["HireDate".to_string()]
        );
    }

    #[test]
    fn an_empty_prefix_offers_every_result_column() {
        assert_eq!(
            TableBrowseFilterBar::result_column_suggestions(&result_columns(), ""),
            result_columns()
        );
    }

    #[test]
    fn result_columns_drop_blanks_and_repeats() {
        // SET HEADING OFF blanks names on the way to the grid, and a join can
        // repeat one; neither should reach the popup twice or empty.
        let columns = vec![
            "DEPTNO".to_string(),
            String::new(),
            "  ".to_string(),
            "deptno".to_string(),
        ];
        assert_eq!(
            TableBrowseFilterBar::result_column_suggestions(&columns, ""),
            vec!["DEPTNO".to_string()]
        );
    }

    fn filter_result(
        db_type: DatabaseType,
        where_expr: &str,
        order_by_expr: &str,
    ) -> Result<String, String> {
        build_logical_sql(
            &target(db_type),
            &TableBrowseClauses::new(where_expr.to_string(), order_by_expr.to_string()),
        )
    }

    /// A filter runs one query or none: a clause that ends the statement it is
    /// spliced into is refused before anything is built.
    #[test]
    fn a_filter_clause_may_not_carry_a_statement_terminator() {
        for db_type in [
            DatabaseType::Oracle,
            DatabaseType::MySQL,
            DatabaseType::MariaDB,
        ] {
            // The reported shape: a terminator with only a comment behind it,
            // which statement splitting drops — so nothing else would object.
            assert!(
                filter_result(db_type, "ABC = 123; -- 123;", "").is_err(),
                "{db_type}"
            );
            assert!(
                filter_result(db_type, "ABC = 123;", "").is_err(),
                "{db_type}"
            );
            assert!(
                filter_result(db_type, "ABC = 1", "ENAME; DROP TABLE T").is_err(),
                "{db_type}"
            );
            // A terminator is only a terminator in code.
            assert!(
                filter_result(db_type, "ENAME = 'a;b'", "").is_ok(),
                "{db_type}"
            );
            assert!(
                filter_result(db_type, "ABC = 1 /* ; */", "").is_ok(),
                "{db_type}"
            );
            assert!(
                filter_result(db_type, "ABC = 1 -- ;", "").is_ok(),
                "{db_type}"
            );
            // And an ordinary filter still builds.
            assert!(
                filter_result(db_type, "DEPTNO = 10", "ENAME DESC").is_ok(),
                "{db_type}"
            );
        }
    }

    /// The Oracle family ends a statement on a line that is nothing but `/`;
    /// the MySQL family does not.
    #[test]
    fn only_the_oracle_family_reads_a_lone_slash_as_a_terminator() {
        assert!(filter_result(DatabaseType::Oracle, "ABC = 1\n/\n", "").is_err());
        assert!(filter_result(DatabaseType::MySQL, "ABC = 1\n/\n", "").is_ok());
        // Division is not a terminator on either.
        assert!(filter_result(DatabaseType::Oracle, "SAL / 2 > 100", "").is_ok());
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn a_filtered_results_columns_lead_the_popup() {
        // The whole point of the fix: without this the popup held keywords
        // only, because no table name can resolve a derived relation.
        let merged = TableBrowseFilterBar::merge_filter_suggestions(
            &result_columns(),
            "E",
            strings(&["ELSE", "EXISTS"]),
        );
        assert_eq!(
            merged,
            strings(&["EMPNO", "ENAME", "ENAME_UPPER", "ELSE", "EXISTS"])
        );
    }

    #[test]
    fn a_browsed_table_keeps_the_engine_answer_untouched() {
        // No result columns means this must be a pass-through, so browsing a
        // real table behaves exactly as it did before the filter feature.
        let from_metadata = strings(&["EMPNO", "ENAME", "ELSE"]);
        assert_eq!(
            TableBrowseFilterBar::merge_filter_suggestions(&[], "E", from_metadata.clone()),
            from_metadata
        );
    }

    #[test]
    fn a_column_the_engine_also_knows_is_not_offered_twice() {
        let merged = TableBrowseFilterBar::merge_filter_suggestions(
            &strings(&["DEPTNO"]),
            "D",
            strings(&["deptno", "DESC"]),
        );
        assert_eq!(merged, strings(&["DEPTNO", "DESC"]));
    }

    #[test]
    fn the_merged_list_respects_the_suggestion_cap() {
        let columns: Vec<String> = (0..crate::ui::intellisense::MAX_SUGGESTIONS + 50)
            .map(|index| format!("C{index}"))
            .collect();
        let merged =
            TableBrowseFilterBar::merge_filter_suggestions(&columns, "C", strings(&["CASE"]));
        assert_eq!(merged.len(), crate::ui::intellisense::MAX_SUGGESTIONS);
    }

    #[test]
    fn a_browsed_table_supplies_no_result_columns_so_metadata_still_answers() {
        // The metadata path must stay the only source for a real relation.
        let target = target(DatabaseType::Oracle);
        assert!(target.result_columns.is_empty());
        assert!(
            TableBrowseFilterBar::result_column_suggestions(&target.result_columns, "E").is_empty()
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn enter_during_ime_composition_does_not_apply_the_filter() {
        assert!(TableBrowseFilterBar::enter_commits_ime_composition(1));
    }

    fn target(db_type: DatabaseType) -> TableBrowseTarget {
        TableBrowseTarget::new(
            db_type,
            Some("APP".to_string()),
            "EMP".to_string(),
            if db_type == DatabaseType::Oracle {
                "APP.EMP".to_string()
            } else {
                "`APP`.`EMP`".to_string()
            },
            "APP.EMP".to_string(),
        )
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn ambiguous_columns_are_named_in_the_filter_tooltips() {
        let _app = fltk::app::App::default();
        let mut bar = TableBrowseFilterBar::new(
            0,
            0,
            600,
            target(DatabaseType::Oracle),
            Arc::new(Mutex::new(IntellisenseData::new())),
            ResultTabId::new(1),
            Arc::new(Mutex::new(None)),
        );

        let before = bar.where_input.tooltip().unwrap_or_default();
        bar.note_ambiguous_columns(&[]);
        assert_eq!(bar.where_input.tooltip().unwrap_or_default(), before);

        bar.note_ambiguous_columns(&["ENAME".to_string(), "SAL".to_string()]);
        for tooltip in [
            bar.where_input.tooltip().unwrap_or_default(),
            bar.order_input.tooltip().unwrap_or_default(),
        ] {
            assert!(tooltip.starts_with(&before));
            assert!(tooltip.contains("ENAME, SAL"));
        }
    }

    #[test]
    fn a_filtered_re_query_is_recognizable_as_this_features_own() {
        let user_sql = "SELECT * FROM EMP";
        assert!(!is_generated_grid_statement(user_sql));

        let filtered = marked_result_filter_sql(&format!(
            "SELECT * FROM (\n{user_sql}\n) sq_src\nWHERE DEPTNO = 10"
        ));
        assert!(is_generated_grid_statement(&filtered));
        // Lazy fetch stays on: only a page statement is materialized.
        assert!(!is_materialized_grid_statement(&filtered));
        assert!(filtered.starts_with("SELECT /*"));
    }

    #[test]
    fn header_clicks_toggle_a_column_between_ascending_and_descending() {
        let first = next_order_by_for_header_sort("", "ENAME");
        assert_eq!(first, "ENAME");
        let second = next_order_by_for_header_sort(&first, "ENAME");
        assert_eq!(second, "ENAME DESC");
        let third = next_order_by_for_header_sort(&second, "ENAME");
        assert_eq!(third, "ENAME");
    }

    #[test]
    fn header_clicks_read_the_order_by_the_user_may_have_typed() {
        // Spelling and spacing of a typed clause do not hide the sort it is.
        assert_eq!(
            next_order_by_for_header_sort("  ename   asc ", "ENAME"),
            "ENAME DESC"
        );
        assert_eq!(
            next_order_by_for_header_sort("ename desc", "ENAME"),
            "ENAME"
        );
        // Another column, or an expression this click cannot claim, restarts.
        assert_eq!(next_order_by_for_header_sort("SAL DESC", "ENAME"), "ENAME");
        assert_eq!(
            next_order_by_for_header_sort("ENAME, SAL", "ENAME"),
            "ENAME"
        );
        assert_eq!(
            next_order_by_for_header_sort("ENAME NULLS FIRST", "ENAME"),
            "ENAME"
        );
    }

    #[test]
    fn every_page_and_count_statement_is_recognizable_as_this_features_own() {
        let request = TableBrowsePageRequest {
            result_tab_id: ResultTabId::new(1),
            target: target(DatabaseType::Oracle),
            clauses: TableBrowseClauses::new("DEPTNO = 10".into(), "ENAME".into()),
            offset: 0,
            page_size: 100,
            navigation: TableBrowseNavigation::Page,
        };

        assert!(is_materialized_grid_statement(&request.page_sql().unwrap()));
        assert!(is_materialized_grid_statement(
            &request.count_sql().unwrap()
        ));
        assert!(!is_materialized_grid_statement(
            &request.logical_sql().unwrap()
        ));
        assert!(!is_materialized_grid_statement("SELECT * FROM EMP"));
        assert!(is_generated_grid_statement(&request.page_sql().unwrap()));
    }

    #[test]
    fn mysql_page_uses_bounded_limit_with_sentinel() {
        let request = TableBrowsePageRequest {
            result_tab_id: ResultTabId::new(1),
            target: target(DatabaseType::MySQL),
            clauses: TableBrowseClauses::new("DEPTNO = 10".into(), "ENAME DESC".into()),
            offset: 500,
            page_size: 500,
            navigation: TableBrowseNavigation::Page,
        };
        let sql = request.page_sql().unwrap();
        assert!(sql.contains("WHERE DEPTNO = 10"));
        assert!(sql.contains("ORDER BY ENAME DESC"));
        assert!(sql.contains("LIMIT 501 OFFSET 500"));
        assert!(sql.contains(TABLE_BROWSE_MATERIALIZE_MARKER));
        assert_eq!(
            crate::ui::sql_editor::query_text::resolve_edit_target_table(&sql).unwrap(),
            "APP.EMP"
        );
    }

    #[test]
    fn oracle_page_uses_11g_rownum_bounds() {
        let request = TableBrowsePageRequest {
            result_tab_id: ResultTabId::new(1),
            target: target(DatabaseType::Oracle),
            clauses: TableBrowseClauses::default(),
            offset: 100,
            page_size: 10,
            navigation: TableBrowseNavigation::Page,
        };
        let sql = request.page_sql().unwrap();
        assert!(sql.contains("ROWNUM <= 111"));
        assert!(sql.contains("SQ_INTERNAL_PAGE_ROW > 100"));
        assert!(sql.contains("SQ_INTERNAL_ROWID"));
        assert!(!sql.contains(" OFFSET "));
    }

    #[test]
    fn oracle_read_only_page_omits_the_rowid_edit_column() {
        let request = TableBrowsePageRequest {
            result_tab_id: ResultTabId::new(1),
            target: target(DatabaseType::Oracle).read_only(),
            clauses: TableBrowseClauses::default(),
            offset: 100,
            page_size: 10,
            navigation: TableBrowseNavigation::Page,
        };
        let sql = request.page_sql().unwrap();
        assert!(sql.contains("ROWNUM <= 111"));
        assert!(sql.contains("SQ_INTERNAL_PAGE_ROW > 100"));
        assert!(!sql.contains("SQ_INTERNAL_ROWID"));
    }

    #[test]
    fn count_omits_order_by() {
        let sql = build_count_sql(
            &target(DatabaseType::MariaDB),
            &TableBrowseClauses::new("ACTIVE = 1".into(), "ID DESC".into()),
        )
        .unwrap();
        assert!(sql.contains("WHERE ACTIVE = 1"));
        assert!(!sql.contains("ORDER BY"));
    }

    #[test]
    fn rejects_multiple_statements_in_clause() {
        let result = build_logical_sql(
            &target(DatabaseType::Oracle),
            &TableBrowseClauses::new("1 = 1; DELETE FROM APP.EMP".into(), String::new()),
        );
        assert!(result.is_err());
    }

    #[test]
    fn last_page_math_handles_empty_exact_and_partial_pages() {
        assert_eq!(last_page_offset(0, 100), 0);
        assert_eq!(last_page_offset(100, 100), 0);
        assert_eq!(last_page_offset(101, 100), 100);
        assert_eq!(last_page_offset(200, 100), 100);
    }

    #[test]
    fn popup_keys_match_query_editor_navigation_and_confirmation() {
        let expected = [
            (Key::Up, TableBrowsePopupKeyAction::SelectPrev),
            (Key::Down, TableBrowsePopupKeyAction::SelectNext),
            (Key::PageUp, TableBrowsePopupKeyAction::SelectPrevPage),
            (Key::PageDown, TableBrowsePopupKeyAction::SelectNextPage),
            (Key::Enter, TableBrowsePopupKeyAction::Confirm),
            (Key::KPEnter, TableBrowsePopupKeyAction::Confirm),
            (Key::Tab, TableBrowsePopupKeyAction::Confirm),
            (Key::Escape, TableBrowsePopupKeyAction::Dismiss),
        ];
        for (key, action) in expected {
            assert_eq!(TableBrowseFilterBar::popup_key_action(key), Some(action));
        }
        assert_eq!(TableBrowseFilterBar::popup_key_action(Key::Left), None);
    }

    #[test]
    fn popup_shortcuts_prevent_secondary_navigation_dispatch() {
        for key in [
            Key::Up,
            Key::Down,
            Key::PageUp,
            Key::PageDown,
            Key::Enter,
            Key::KPEnter,
            Key::Tab,
        ] {
            assert!(TableBrowseFilterBar::should_consume_popup_shortcut(
                true, key
            ));
            assert!(!TableBrowseFilterBar::should_consume_popup_shortcut(
                false, key
            ));
        }
        assert!(!TableBrowseFilterBar::should_consume_popup_shortcut(
            true,
            Key::Escape
        ));
    }

    #[test]
    fn popup_unfocus_keeps_internal_click_available_for_selection() {
        assert!(!TableBrowseFilterBar::should_hide_popup_on_unfocus(
            true, true
        ));
        assert!(TableBrowseFilterBar::should_hide_popup_on_unfocus(
            true, false
        ));
        assert!(!TableBrowseFilterBar::should_hide_popup_on_unfocus(
            false, false
        ));
    }

    #[test]
    fn popup_modifier_handling_matches_query_editor() {
        for key in [
            Key::ShiftL,
            Key::ShiftR,
            Key::ControlL,
            Key::ControlR,
            Key::AltL,
            Key::AltR,
            Key::MetaL,
            Key::MetaR,
        ] {
            assert!(TableBrowseFilterBar::should_hide_popup_on_modifier_keydown(
                true, key
            ));
            assert!(TableBrowseFilterBar::is_modifier_key(key));
        }
        assert!(!TableBrowseFilterBar::should_hide_popup_on_modifier_keydown(true, Key::CapsLock));
        assert!(TableBrowseFilterBar::is_modifier_key(Key::CapsLock));
    }

    #[test]
    fn completion_stays_hidden_inside_literals_and_comments() {
        for db_type in [
            DatabaseType::Oracle,
            DatabaseType::MySQL,
            DatabaseType::MariaDB,
        ] {
            for value in [
                "topic='",
                "topic='news",
                "topic=q'[news",
                "topic=1 /* note",
                "topic=1 -- note",
            ] {
                assert!(TableBrowseFilterBar::completion_is_suppressed_at_cursor(
                    value,
                    value.len(),
                    db_type,
                ));
            }
            let value = "topic='news' AND top";
            assert!(!TableBrowseFilterBar::completion_is_suppressed_at_cursor(
                value,
                value.len(),
                db_type,
            ));
        }
    }

    #[test]
    fn automatic_completion_requires_at_least_one_prefix_character() {
        assert!(!TableBrowseFilterBar::should_open_completion("", false));
        assert!(TableBrowseFilterBar::should_open_completion("E", false));
        assert!(TableBrowseFilterBar::should_open_completion("", true));
    }

    #[test]
    fn table_browse_callback_is_unlocked_and_restored_after_panic() {
        let callback: TableBrowseExecuteCallback = Arc::new(Mutex::new(None));
        let callback_for_assert = callback.clone();
        let calls = Arc::new(Mutex::new(0usize));
        let calls_for_callback = calls.clone();
        *callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(move |_| {
            assert!(callback_for_assert.try_lock().is_ok());
            let mut calls = calls_for_callback
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *calls += 1;
            if *calls == 1 {
                panic!("expected callback panic");
            }
            Ok(())
        }));

        let request =
            TableBrowsePageRequest::first(ResultTabId::new(1), target(DatabaseType::MySQL));
        assert!(invoke_table_browse_execute_callback(&callback, request.clone()).is_err());
        assert!(invoke_table_browse_execute_callback(&callback, request).is_ok());
        assert_eq!(
            *calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            2
        );
    }
}
