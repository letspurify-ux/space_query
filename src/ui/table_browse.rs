use fltk::{
    app,
    button::Button,
    enums::{Align, CallbackTrigger, Event, FrameType, Key, Shortcut},
    frame::Frame,
    group::Group,
    input::Input,
    prelude::*,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use crate::db::{DatabaseType, DbTableBrowsePagination, QueryExecutor};
use crate::ui::intellisense::{get_word_at_cursor, IntellisenseData, IntellisensePopup};
use crate::ui::result_tabs::ResultTabId;
use crate::ui::theme;
use crate::utils::arithmetic::safe_div;

pub(crate) const TABLE_BROWSE_MATERIALIZE_MARKER: &str = "SQ_INTERNAL_TABLE_BROWSE";
pub(crate) const TABLE_BROWSE_PAGE_COLUMN: &str = "SQ_INTERNAL_PAGE_ROW";
pub(crate) const TABLE_BROWSE_DEFAULT_PAGE_SIZE: usize = 500;
pub(crate) const TABLE_BROWSE_FILTER_HEIGHT: i32 = 42;

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
        }
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

fn marked_materialized_sql(sql: &str) -> String {
    let trimmed_start = sql.len().saturating_sub(sql.trim_start().len());
    let trimmed = sql.trim_start();
    if trimmed
        .get(..6)
        .is_some_and(|head| head.eq_ignore_ascii_case("SELECT"))
    {
        format!(
            "{}SELECT /* {TABLE_BROWSE_MATERIALIZE_MARKER} */{}",
            &sql[..trimmed_start],
            &trimmed[6..]
        )
    } else {
        format!("/* {TABLE_BROWSE_MATERIALIZE_MARKER} */\n{sql}")
    }
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
            let rowid_sql = QueryExecutor::maybe_inject_rowid_for_editing(&logical_sql);
            let rowid_sql = QueryExecutor::rowid_safe_execution_sql(&logical_sql, &rowid_sql);
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
        input.handle(move |input, event| match event {
            Event::KeyDown => {
                let key =
                    Self::shortcut_key_for_layout(app::event_key(), app::event_original_key());
                let shortcut = app::event_state();
                let ctrl_or_cmd =
                    shortcut.contains(Shortcut::Ctrl) || shortcut.contains(Shortcut::Command);
                let popup_visible = popup
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .is_visible();

                if ctrl_or_cmd && key == Key::from_char(' ') {
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
                                let _ = input.take_focus();
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
        let mut callback_fn = callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let result = callback_fn
            .as_mut()
            .map(|callback| callback(request))
            .unwrap_or_else(|| Err("Table browse callback is unavailable.".to_string()));
        let mut callback_guard = callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if callback_guard.is_none() {
            *callback_guard = callback_fn;
        }
        drop(callback_guard);
        if let Err(message) = result {
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
        if !input.has_focus() {
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
        let suggestions = intellisense_data
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
        } else {
            self.hide_popups();
            self.where_input.deactivate();
            self.order_input.deactivate();
            self.where_clear.deactivate();
            self.order_clear.deactivate();
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
