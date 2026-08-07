use fltk::{
    app,
    enums::{Align, CallbackReason, CallbackTrigger, Event, FrameType, Key},
    group::{Group, Tabs, TabsOverflow},
    input::Input,
    prelude::*,
    text::{TextBuffer, TextDisplay},
};
use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::db::query::result_messages;
use crate::db::{ColumnInfo, ExecutionOrigin, QueryResult, ResultEditDescriptor, SqlValueKind};
use crate::ui::constants;
use crate::ui::font_settings::{
    configured_result_font_size, configured_result_profile, FontProfile,
};
use crate::ui::grid_sql_export::GridSqlSelection;
use crate::ui::result_export::{ExportDestination, ExportFormat, ExportScope};
use crate::ui::result_table::{
    ExportRequest, LazyFetchCallback, ResultGridEditExecuteCallback, ResultGridSqlExecuteCallback,
    ResultPageNavigationOutcome, ResultTableContextActionCallback,
};
use crate::ui::tab_strip;
use crate::ui::table_browse::{
    invoke_table_browse_execute_callback, TableBrowseExecuteCallback, TableBrowseFilterBar,
    TableBrowseNavigation, TableBrowsePageRequest, TableBrowseTarget, TABLE_BROWSE_FILTER_HEIGHT,
};
use crate::ui::text_buffer_access;
use crate::ui::theme;
use crate::ui::{IntellisensePopup, ResultTableWidget};
use crate::utils::arithmetic::safe_div;

type ResultTabsChangeCallback = Box<dyn FnMut()>;
type ResultTabsCloseCallback = Box<dyn FnMut(ResultTabCloseTarget)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ResultTabId(u64);

impl ResultTabId {
    pub(crate) fn new(value: u64) -> Self {
        Self(value.max(1))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResultTabCloseTarget {
    Result(ResultTabId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultMessageKind {
    Info,
    Error,
}

#[derive(Clone)]
pub struct ResultTabsWidget {
    tabs: Tabs,
    data_tabs: Tabs,
    script_tabs: Tabs,
    messages_tabs: Tabs,
    sections: ResultSections,
    data: Arc<Mutex<Vec<ResultTab>>>,
    active_index: Arc<Mutex<Option<usize>>>,
    script_output: Arc<Mutex<TextPane>>,
    script_errors: Arc<Mutex<TextPane>>,
    dbms_output: Arc<Mutex<TextPane>>,
    messages_info: Arc<Mutex<TextPane>>,
    messages_errors: Arc<Mutex<TextPane>>,
    next_result_tab_id: Arc<Mutex<u64>>,
    font_profile: Arc<Mutex<FontProfile>>,
    font_size: Arc<Mutex<u32>>,
    max_cell_display_chars: Arc<Mutex<usize>>,
    execute_sql_callback: Arc<Mutex<Option<ResultGridSqlExecuteCallback>>>,
    execute_edit_callback: Arc<Mutex<Option<ResultGridEditExecuteCallback>>>,
    lazy_fetch_callback: LazyFetchCallback,
    context_action_callback: ResultTableContextActionCallback,
    table_browse_callback: TableBrowseExecuteCallback,
    on_change_callback: Arc<Mutex<Option<ResultTabsChangeCallback>>>,
    on_close_callback: Arc<Mutex<Option<ResultTabsCloseCallback>>>,
    suppress_pointer_event_depth: Arc<Mutex<u32>>,
    data_tab_strip_state: Arc<Mutex<tab_strip::TabStripState>>,
    execution_origin: Arc<Mutex<Option<ExecutionOrigin>>>,
}

#[derive(Clone)]
struct ResultTab {
    id: ResultTabId,
    title: String,
    group: Group,
    table: ResultTableWidget,
    status: ResultTabStatus,
    row_count: usize,
    origin: Option<ExecutionOrigin>,
    kind: ResultTabKind,
    filter_bar: Option<TableBrowseFilterBar>,
}

#[derive(Clone)]
enum ResultTabKind {
    Query,
    TableBrowse(Box<TableBrowseState>),
}

#[derive(Clone)]
struct TableBrowseState {
    applied_request: TableBrowsePageRequest,
    pending_request: Option<TableBrowsePageRequest>,
    has_next: bool,
    loading: bool,
    last_success: Option<QueryResult>,
    last_edit_descriptor: Option<ResultEditDescriptor>,
}

impl TableBrowseState {
    fn normalize_request(&self, request: &mut TableBrowsePageRequest) {
        request.target = self.applied_request.target.clone();
        if request.page_size == 0 {
            request.page_size = self.applied_request.page_size;
        }
    }
}

#[derive(Clone)]
struct ResultSections {
    data_grid: Group,
    script_output: Group,
    dbms_output: Group,
    messages: Group,
}

#[derive(Clone)]
struct TextPane {
    group: Group,
    display: TextDisplay,
    buffer: TextBuffer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResultTabStatus {
    Running,
    Fetching,
    Waiting,
    Canceling,
    Done,
    Error,
    Cancelled,
}

impl ResultTabStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Fetching => "Fetching",
            Self::Waiting => "Waiting",
            Self::Canceling => "Canceling",
            Self::Done => "Done",
            Self::Error => "Error",
            Self::Cancelled => "Cancelled",
        }
    }

    pub(crate) fn status_bar_message(self) -> &'static str {
        match self {
            Self::Running => "Running query...",
            Self::Fetching => "Fetching rows",
            Self::Waiting => "Waiting for lazy fetch",
            Self::Canceling => "Canceling",
            Self::Done => "Done",
            Self::Error => "Error",
            Self::Cancelled => "Cancelled",
        }
    }

    pub(crate) fn status_bar_message_with_rows(self, row_count: usize) -> String {
        if self == Self::Fetching {
            format!("{}: {}", self.status_bar_message(), row_count)
        } else {
            self.status_bar_message().to_string()
        }
    }

    fn for_stream_update(current: Self) -> Self {
        match current {
            Self::Canceling | Self::Cancelled | Self::Done | Self::Error => current,
            Self::Running | Self::Fetching | Self::Waiting => Self::Fetching,
        }
    }

    fn is_cancelled_message(message: &str) -> bool {
        let trimmed = message.trim();
        let normalized = if trimmed
            .get(.."Error:".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Error:"))
        {
            &trimmed["Error:".len()..]
        } else {
            trimmed
        }
        .trim();
        normalized.eq_ignore_ascii_case(result_messages::QUERY_CANCELLED)
            || normalized.eq_ignore_ascii_case("Query canceled")
    }

    pub(crate) fn from_query_result(result: &crate::db::QueryResult) -> Self {
        if result.success {
            Self::Done
        } else if Self::is_cancelled_message(&result.message) {
            Self::Cancelled
        } else {
            Self::Error
        }
    }
}

struct PointerEventSuppressGuard {
    counter: Arc<Mutex<u32>>,
}

impl PointerEventSuppressGuard {
    fn new(counter: Arc<Mutex<u32>>) -> Self {
        {
            let mut guard = counter
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *guard = guard.saturating_add(1);
        }
        Self { counter }
    }
}

impl Drop for PointerEventSuppressGuard {
    fn drop(&mut self) {
        let mut guard = self
            .counter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = guard.saturating_sub(1);
    }
}

impl ResultTabsWidget {
    const INNER_TAB_TOP_GAP: i32 = 10;

    fn top_level_tab_labels() -> [&'static str; 4] {
        [
            " Data Grid ",
            " Script Output ",
            " DBMS Output ",
            " Messages ",
        ]
    }

    fn script_output_tab_labels() -> [&'static str; 2] {
        [" Output ", " Errors "]
    }

    fn messages_tab_labels() -> [&'static str; 2] {
        [" Info ", " Errors "]
    }

    fn panic_payload_to_string(payload: &(dyn Any + Send)) -> String {
        if let Some(message) = payload.downcast_ref::<&str>() {
            (*message).to_string()
        } else if let Some(message) = payload.downcast_ref::<String>() {
            message.clone()
        } else {
            "unknown panic payload".to_string()
        }
    }

    fn invoke_change_callback(callback: &mut ResultTabsChangeCallback) {
        let callback_result = panic::catch_unwind(AssertUnwindSafe(callback));
        if let Err(payload) = callback_result {
            let panic_payload = Self::panic_payload_to_string(payload.as_ref());
            crate::utils::logging::log_error(
                "result_tabs::callback",
                &format!("result tabs change callback panicked: {panic_payload}"),
            );
            eprintln!("result tabs change callback panicked: {panic_payload}");
        }
    }

    fn invoke_close_callback(callback: &mut ResultTabsCloseCallback, target: ResultTabCloseTarget) {
        let callback_result = panic::catch_unwind(AssertUnwindSafe(|| callback(target)));
        if let Err(payload) = callback_result {
            let panic_payload = Self::panic_payload_to_string(payload.as_ref());
            crate::utils::logging::log_error(
                "result_tabs::callback",
                &format!("result tab close callback panicked: {panic_payload}"),
            );
            eprintln!("result tab close callback panicked: {panic_payload}");
        }
    }

    fn fire_on_change_callback(&self) {
        let mut callback = self
            .on_change_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(callback_fn) = callback.as_mut() {
            Self::invoke_change_callback(callback_fn);
        }
        *self
            .on_change_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = callback;
    }

    fn fire_on_change_with(callback_ref: &Arc<Mutex<Option<ResultTabsChangeCallback>>>) {
        let mut callback = callback_ref
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(callback_fn) = callback.as_mut() {
            Self::invoke_change_callback(callback_fn);
        }
        *callback_ref
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = callback;
    }

    fn fire_on_close_with(
        callback_ref: &Arc<Mutex<Option<ResultTabsCloseCallback>>>,
        target: ResultTabCloseTarget,
    ) {
        let mut callback = callback_ref
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(callback_fn) = callback.as_mut() {
            Self::invoke_close_callback(callback_fn, target);
        }
        *callback_ref
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = callback;
    }

    fn invoke_lazy_fetch_callback(
        callback_ref: &LazyFetchCallback,
        session_id: u64,
        request: crate::ui::sql_editor::LazyFetchRequest,
    ) -> bool {
        let mut callback = callback_ref
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let accepted = callback
            .as_mut()
            .is_some_and(|callback_fn| callback_fn(session_id, request));
        let mut callback_guard = callback_ref
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if callback_guard.is_none() {
            *callback_guard = callback;
        }
        accepted
    }

    fn content_bounds(tabs: &Tabs) -> (i32, i32, i32, i32) {
        // Keep a stable tab-header height regardless of surrounding splitter drags.
        // This avoids top/bottom header bar height jitter while panes are resized.
        let x = tabs.x();
        let y = tabs.y() + constants::TAB_HEADER_HEIGHT;
        let w = tabs.w();
        let h = tabs.h() - constants::TAB_HEADER_HEIGHT;
        (x, y, w.max(1), h.max(1))
    }

    fn layout_children(tabs: &Tabs) {
        let (x, y, w, h) = Self::content_bounds(tabs);
        for child in tabs.clone().into_iter() {
            if let Some(mut group) = child.as_group() {
                group.resize(x, y, w, h);
            }
        }
    }

    fn layout_active_tab_child(tabs: &mut Tabs) {
        // Reposition *every* child to the same content bounds, not just the active
        // one. FLTK's Fl_Tabs derives its header height from the minimum top-gap
        // across all children (child.y() - tabs.y()); leaving inactive children at
        // a stale y after the tabs widget moves during a splitter drag makes that
        // minimum diverge from TAB_HEADER_HEIGHT, so the header shrinks or grows.
        Self::layout_children(tabs);
        tabs.redraw();
    }

    fn inner_tabs_bounds(x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
        let gap = if h > 1 {
            Self::INNER_TAB_TOP_GAP.min(h - 1)
        } else {
            0
        };
        (x, y + gap, w.max(1), (h - gap).max(1))
    }

    fn layout_inner_tabs(section: &Group, tabs: &mut Tabs) {
        if section.was_deleted() || tabs.was_deleted() {
            return;
        }
        let (x, y, w, h) =
            Self::inner_tabs_bounds(section.x(), section.y(), section.w(), section.h());
        tabs.resize(x, y, w, h);
        Self::layout_children(tabs);
    }

    fn layout_active_inner_tabs(section: &Group, tabs: &mut Tabs) {
        if section.was_deleted() || tabs.was_deleted() {
            return;
        }
        let (x, y, w, h) =
            Self::inner_tabs_bounds(section.x(), section.y(), section.w(), section.h());
        tabs.resize(x, y, w, h);
        Self::layout_active_tab_child(tabs);
    }

    fn layout_text_pane_in_tabs(tabs: &Tabs, pane: &mut TextPane) {
        let (x, y, w, h) = Self::content_bounds(tabs);
        Self::layout_text_pane_at(pane, x, y, w, h);
    }

    fn layout_text_pane_in_group(group: &Group, pane: &mut TextPane) {
        if group.was_deleted() {
            return;
        }
        Self::layout_text_pane_at(pane, group.x(), group.y(), group.w(), group.h());
    }

    fn layout_text_pane_at(pane: &mut TextPane, x: i32, y: i32, w: i32, h: i32) {
        if pane.group.was_deleted() || pane.display.was_deleted() {
            return;
        }
        pane.group.resize(x, y, w.max(1), h.max(1));
        let padding = constants::SCRIPT_OUTPUT_PADDING;
        pane.display.resize(
            x + padding,
            y + padding,
            (w - padding * 2).max(10),
            (h - padding * 2).max(10),
        );
    }

    fn should_suppress_pointer_event(depth: &Arc<Mutex<u32>>, ev: Event) -> bool {
        matches!(
            ev,
            Event::Enter
                | Event::Move
                | Event::Push
                | Event::Drag
                | Event::Released
                | Event::Leave
                | Event::MouseWheel
        ) && *depth
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            > 0
    }

    fn should_consume_empty_tab_pointer_event(child_count: i32, ev: Event) -> bool {
        child_count == 0
            && matches!(
                ev,
                Event::Enter
                    | Event::Move
                    | Event::Push
                    | Event::Drag
                    | Event::Released
                    | Event::Leave
                    | Event::MouseWheel
            )
    }

    fn sync_data_tab_strip_overflow_mode_for(
        tabs: &mut Tabs,
        tab_strip_state: &Arc<Mutex<tab_strip::TabStripState>>,
    ) {
        let _ = tab_strip::try_with_state(tab_strip_state, |state| {
            tab_strip::sync_overflow_mode(tabs, state, constants::TAB_HEADER_HEIGHT);
        });
    }

    fn record_data_tab_strip_selection_for(
        tabs: &mut Tabs,
        tab_strip_state: &Arc<Mutex<tab_strip::TabStripState>>,
    ) {
        let _ = tab_strip::try_with_state(tab_strip_state, |state| {
            tab_strip::record_selected_tab(tabs, state, constants::TAB_HEADER_HEIGHT);
        });
    }

    fn record_data_tab_strip_removal_for(
        group: &Group,
        tab_strip_state: &Arc<Mutex<tab_strip::TabStripState>>,
    ) {
        let _ = tab_strip::try_with_state(tab_strip_state, |state| {
            tab_strip::record_removed_tab(state, group.as_widget_ptr() as usize);
        });
    }

    fn sync_data_tab_strip_overflow_mode(&mut self) {
        Self::sync_data_tab_strip_overflow_mode_for(
            &mut self.data_tabs,
            &self.data_tab_strip_state,
        );
    }

    fn sync_data_tab_strip_overflow_mode_after_close(&self) {
        let mut tabs = self.data_tabs.clone();
        let tab_strip_state = self.data_tab_strip_state.clone();
        crate::ui::ui_timeout::schedule(0.0, move || {
            if tabs.was_deleted() {
                return;
            }
            Self::sync_data_tab_strip_overflow_mode_for(&mut tabs, &tab_strip_state);
            tabs.redraw();
        });
    }

    fn maybe_shrink_tab_storage(data: &mut Vec<ResultTab>) {
        // Avoid frequent shrinking; only compact when capacity is materially over-provisioned.
        let len = data.len();
        let capacity = data.capacity();
        if len == 0 || (capacity > 0 && len.saturating_mul(2) < capacity) {
            data.shrink_to_fit();
        }
    }

    fn buffer_ends_with_newline(buffer: &TextBuffer) -> bool {
        let len = buffer.length();
        if len <= 0 {
            return false;
        }
        text_buffer_access::text_range(buffer, None, len - 1, len) == "\n"
    }

    fn trim_script_output_buffer(buffer: &mut TextBuffer) {
        let max_chars = constants::SCRIPT_OUTPUT_MAX_CHARS;
        let target_chars = constants::SCRIPT_OUTPUT_TRIM_TARGET_CHARS.min(max_chars);
        let len = buffer.length().max(0) as usize;
        if len <= max_chars {
            return;
        }

        let remove_upto = len.saturating_sub(target_chars);
        if remove_upto == 0 {
            return;
        }

        let prefix = text_buffer_access::text_range(buffer, None, 0, remove_upto as i32);
        let cut = prefix.rfind('\n').map(|idx| idx + 1).unwrap_or(remove_upto);
        if cut > 0 {
            buffer.remove(0, cut as i32);
        }
    }

    fn result_tab_title(_index: usize, status: ResultTabStatus, row_count: usize) -> String {
        format!("{} ({})", status.label(), row_count)
    }

    fn result_tab_label(index: usize, status: ResultTabStatus, row_count: usize) -> String {
        format!(" {} ", Self::result_tab_title(index, status, row_count))
    }

    fn result_tab_label_for_title(
        title: &str,
        index: usize,
        status: ResultTabStatus,
        row_count: usize,
    ) -> String {
        let title = title.trim();
        if title.is_empty() || title == "Result" {
            Self::result_tab_label(index, status, row_count)
        } else {
            format!(" {} · {} ({}) ", title, status.label(), row_count)
        }
    }

    fn table_browse_page_number(offset: u64, page_size: usize) -> u64 {
        let page_size = u64::try_from(page_size).unwrap_or(u64::MAX).max(1);
        safe_div(offset, page_size).saturating_add(1)
    }

    fn table_browse_tab_label_for_title(title: &str, page: u64, row_count: usize) -> String {
        let title = title.trim();
        if title.is_empty() || title == "Result" {
            format!(" Page {page} ({row_count}) ")
        } else {
            format!(" {title} · Page {page} ({row_count}) ")
        }
    }

    fn tabs_contains_group(tabs: &Tabs, group: &Group) -> bool {
        !tabs.was_deleted() && !group.was_deleted() && tabs.find(group) < tabs.children()
    }

    fn top_group_is_current(&self, group: &Group) -> bool {
        if self.tabs.was_deleted() || group.was_deleted() {
            return false;
        }
        self.tabs
            .value()
            .is_some_and(|current| current.as_widget_ptr() == group.as_widget_ptr())
    }

    fn select_top_group(&mut self, group: &Group) {
        if self.tabs.was_deleted() || group.was_deleted() {
            return;
        }
        let _ = self.tabs.set_value(group);
    }

    fn select_text_tab(tabs: &mut Tabs, pane: &Arc<Mutex<TextPane>>) {
        if tabs.was_deleted() {
            return;
        }
        let group = pane
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .group
            .clone();
        if !group.was_deleted() && Self::tabs_contains_group(tabs, &group) {
            let _ = tabs.set_value(&group);
        }
    }

    fn update_tab_group_label(
        &mut self,
        title: &str,
        index: usize,
        mut group: Group,
        status: ResultTabStatus,
        row_count: usize,
    ) {
        if self.data_tabs.was_deleted() || group.was_deleted() {
            return;
        }
        group.set_label(&Self::result_tab_label_for_title(
            title, index, status, row_count,
        ));
        group.redraw();
        self.sync_data_tab_strip_overflow_mode();
        self.data_tabs.redraw();
    }

    fn update_table_browse_tab_group_label(
        &mut self,
        title: &str,
        page: u64,
        row_count: usize,
        mut group: Group,
    ) {
        if self.data_tabs.was_deleted() || group.was_deleted() {
            return;
        }
        group.set_label(&Self::table_browse_tab_label_for_title(
            title, page, row_count,
        ));
        group.redraw();
        self.sync_data_tab_strip_overflow_mode();
        self.data_tabs.redraw();
    }

    fn set_result_tab_state(
        &mut self,
        index: usize,
        status: ResultTabStatus,
        row_count: usize,
    ) -> Option<(Group, ResultTableWidget)> {
        let tab_parts = {
            let mut data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            data.get_mut(index).map(|tab| {
                tab.status = status;
                tab.row_count = row_count;
                (tab.title.clone(), tab.group.clone(), tab.table.clone())
            })
        };
        if let Some((title, group, _)) = tab_parts.as_ref() {
            self.update_tab_group_label(title, index, group.clone(), status, row_count);
        }
        tab_parts.map(|(_, group, table)| (group, table))
    }

    fn style_tabs(tabs: &mut Tabs) {
        tabs.set_color(theme::panel_bg());
        tabs.set_selection_color(theme::selection_soft());
        tabs.set_frame(FrameType::RFlatBox);
        tabs.set_label_color(theme::text_secondary());
        tabs.set_label_size((constants::TAB_HEADER_HEIGHT - 8).max(8));
        tabs.set_tab_align(Align::Center);
        tabs.handle_overflow(TabsOverflow::Compress);
    }

    fn create_section_group(x: i32, y: i32, w: i32, h: i32, label: &str) -> Group {
        let mut group = Group::new(x, y, w, h, None).with_label(label);
        group.set_color(theme::panel_bg());
        group.set_selection_color(theme::panel_bg());
        group.set_label_color(theme::text_secondary());
        group.set_align(Align::Center | Align::Inside);
        group
    }

    fn create_text_pane(
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        label: &str,
        profile: FontProfile,
        size: u32,
    ) -> TextPane {
        let group = Self::create_section_group(x, y, w, h, label);
        group.begin();
        let padding = constants::SCRIPT_OUTPUT_PADDING;
        let mut display = TextDisplay::new(
            x + padding,
            y + padding,
            (w - padding * 2).max(10),
            (h - padding * 2).max(10),
            None,
        );
        display.set_color(theme::panel_bg());
        display.set_text_color(theme::text_primary());
        display.set_text_font(profile.normal);
        display.set_text_size(size as i32);
        let mut buffer = TextBuffer::default();
        buffer.set_text("");
        display.set_buffer(buffer.clone());
        theme::style_text_display_scrollbars(&display);
        group.resizable(&display);
        group.end();
        TextPane {
            group,
            display,
            buffer,
        }
    }

    fn with_text_panes<F>(&self, mut f: F)
    where
        F: FnMut(&Arc<Mutex<TextPane>>),
    {
        f(&self.script_output);
        f(&self.script_errors);
        f(&self.dbms_output);
        f(&self.messages_info);
        f(&self.messages_errors);
    }

    fn append_lines_to_pane(pane: &Arc<Mutex<TextPane>>, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        let mut pane = pane.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut buffer = pane.buffer.clone();
        let has_prefix_newline = buffer.length() > 0 && !Self::buffer_ends_with_newline(&buffer);
        let mut append_capacity = lines.iter().map(|line| line.len() + 1).sum::<usize>();
        if has_prefix_newline {
            append_capacity = append_capacity.saturating_add(1);
        }
        let mut appended = String::with_capacity(append_capacity);
        if has_prefix_newline {
            appended.push('\n');
        }
        for line in lines {
            appended.push_str(line);
            appended.push('\n');
        }
        buffer.append(&appended);
        Self::trim_script_output_buffer(&mut buffer);
        let end_pos = buffer.length();
        pane.display.set_insert_position(end_pos);
        pane.display.show_insert_position();
    }

    fn set_pane_text(pane: &Arc<Mutex<TextPane>>, text: &str) {
        let mut pane = pane.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut buffer = TextBuffer::default();
        buffer.set_text(text);
        pane.display.set_buffer(buffer.clone());
        pane.buffer = buffer;
        pane.display.scroll(0, 0);
        pane.display.redraw();
    }

    fn clear_pane(pane: &Arc<Mutex<TextPane>>) {
        Self::set_pane_text(pane, "");
    }

    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        // Use explicit dimensions to avoid "center of requires the size of the
        // widget to be known" panic that occurs with default_fill()
        let mut tabs = Tabs::new(x, y, w, h, None);
        Self::style_tabs(&mut tabs);

        let data = Arc::new(Mutex::new(Vec::<ResultTab>::new()));
        let active_index = Arc::new(Mutex::new(None));
        let next_result_tab_id = Arc::new(Mutex::new(1u64));
        let font_profile = Arc::new(Mutex::new(configured_result_profile()));
        let font_size = Arc::new(Mutex::new(configured_result_font_size()));
        let max_cell_display_chars = Arc::new(Mutex::new(
            constants::RESULT_CELL_MAX_DISPLAY_CHARS_DEFAULT as usize,
        ));
        let execute_sql_callback: Arc<Mutex<Option<ResultGridSqlExecuteCallback>>> =
            Arc::new(Mutex::new(None));
        let execute_edit_callback: Arc<Mutex<Option<ResultGridEditExecuteCallback>>> =
            Arc::new(Mutex::new(None));
        let lazy_fetch_callback: LazyFetchCallback = Arc::new(Mutex::new(None));
        let context_action_callback: ResultTableContextActionCallback = Arc::new(Mutex::new(None));
        let table_browse_callback: TableBrowseExecuteCallback = Arc::new(Mutex::new(None));
        let on_change_callback: Arc<Mutex<Option<ResultTabsChangeCallback>>> =
            Arc::new(Mutex::new(None));
        let on_close_callback: Arc<Mutex<Option<ResultTabsCloseCallback>>> =
            Arc::new(Mutex::new(None));
        let suppress_pointer_event_depth = Arc::new(Mutex::new(0u32));
        let data_tab_strip_state = Arc::new(Mutex::new(tab_strip::TabStripState::default()));
        let execution_origin = Arc::new(Mutex::new(None));

        tabs.begin();
        let (section_x, section_y, section_w, section_h) = Self::content_bounds(&tabs);
        let text_profile = *font_profile
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let text_size = *font_size
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let [data_grid_label, script_output_label, dbms_output_label, messages_label] =
            Self::top_level_tab_labels();
        let [script_output_output_label, script_output_errors_label] =
            Self::script_output_tab_labels();
        let [messages_info_label, messages_errors_label] = Self::messages_tab_labels();
        let (inner_tabs_x, inner_tabs_y, inner_tabs_w, inner_tabs_h) =
            Self::inner_tabs_bounds(section_x, section_y, section_w, section_h);

        let data_grid_section =
            Self::create_section_group(section_x, section_y, section_w, section_h, data_grid_label);
        data_grid_section.begin();
        let mut data_tabs = Tabs::new(inner_tabs_x, inner_tabs_y, inner_tabs_w, inner_tabs_h, None);
        Self::style_tabs(&mut data_tabs);
        data_tabs.end();
        data_grid_section.resizable(&data_tabs);
        data_grid_section.end();

        let script_section = Self::create_section_group(
            section_x,
            section_y,
            section_w,
            section_h,
            script_output_label,
        );
        script_section.begin();
        let mut script_tabs =
            Tabs::new(inner_tabs_x, inner_tabs_y, inner_tabs_w, inner_tabs_h, None);
        Self::style_tabs(&mut script_tabs);
        script_tabs.begin();
        let (text_x, text_y, text_w, text_h) = Self::content_bounds(&script_tabs);
        let script_output_pane = Self::create_text_pane(
            text_x,
            text_y,
            text_w,
            text_h,
            script_output_output_label,
            text_profile,
            text_size,
        );
        let script_errors_pane = Self::create_text_pane(
            text_x,
            text_y,
            text_w,
            text_h,
            script_output_errors_label,
            text_profile,
            text_size,
        );
        script_tabs.end();
        script_section.resizable(&script_tabs);
        script_section.end();

        let dbms_section = Self::create_section_group(
            section_x,
            section_y,
            section_w,
            section_h,
            dbms_output_label,
        );
        dbms_section.begin();
        let dbms_output_pane = Self::create_text_pane(
            section_x,
            section_y,
            section_w,
            section_h,
            "",
            text_profile,
            text_size,
        );
        dbms_section.resizable(&dbms_output_pane.group);
        dbms_section.end();

        let messages_section =
            Self::create_section_group(section_x, section_y, section_w, section_h, messages_label);
        messages_section.begin();
        let mut messages_tabs =
            Tabs::new(inner_tabs_x, inner_tabs_y, inner_tabs_w, inner_tabs_h, None);
        Self::style_tabs(&mut messages_tabs);
        messages_tabs.begin();
        let (message_x, message_y, message_w, message_h) = Self::content_bounds(&messages_tabs);
        let messages_info_pane = Self::create_text_pane(
            message_x,
            message_y,
            message_w,
            message_h,
            messages_info_label,
            text_profile,
            text_size,
        );
        let messages_errors_pane = Self::create_text_pane(
            message_x,
            message_y,
            message_w,
            message_h,
            messages_errors_label,
            text_profile,
            text_size,
        );
        messages_tabs.end();
        messages_section.resizable(&messages_tabs);
        messages_section.end();

        tabs.end();
        let _ = tabs.set_value(&data_grid_section);
        let _ = script_tabs.set_value(&script_output_pane.group);
        let _ = messages_tabs.set_value(&messages_info_pane.group);

        let sections = ResultSections {
            data_grid: data_grid_section,
            script_output: script_section,
            dbms_output: dbms_section,
            messages: messages_section,
        };

        let script_output = Arc::new(Mutex::new(script_output_pane));
        let script_errors = Arc::new(Mutex::new(script_errors_pane));
        let dbms_output = Arc::new(Mutex::new(dbms_output_pane));
        let messages_info = Arc::new(Mutex::new(messages_info_pane));
        let messages_errors = Arc::new(Mutex::new(messages_errors_pane));

        let data_for_cb = data.clone();
        let active_for_cb = active_index.clone();
        let on_change_for_cb = on_change_callback.clone();
        let data_tab_strip_state_for_cb = data_tab_strip_state.clone();
        data_tabs.set_callback(move |t| {
            Self::record_data_tab_strip_selection_for(t, &data_tab_strip_state_for_cb);
            if let Some(widget) = t.value() {
                let ptr = widget.as_widget_ptr();
                let index = data_for_cb
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .iter()
                    .position(|tab| tab.group.as_widget_ptr() == ptr);
                *active_for_cb
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = index;
                Self::fire_on_change_with(&on_change_for_cb);
            }
            Self::layout_active_tab_child(t);
        });

        let on_change_for_top_cb = on_change_callback.clone();
        tabs.set_callback(move |_| {
            Self::fire_on_change_with(&on_change_for_top_cb);
        });

        let suppress_pointer_for_cb = suppress_pointer_event_depth.clone();
        let tabs_for_key = tabs.clone();
        // Filter header wheel events before FLTK can mutate its tab offset.
        // Wheel events in result content must continue to reach the active pane.
        tabs.super_handle_first(false);
        tabs.handle(move |tabs, ev| {
            if Self::should_suppress_pointer_event(&suppress_pointer_for_cb, ev) {
                return true;
            }
            if Self::should_consume_empty_tab_pointer_event(tabs.children(), ev) {
                return true;
            }
            if matches!(ev, Event::MouseWheel)
                && tab_strip::should_consume_mouse_wheel_for_tabs(
                    tabs,
                    constants::TAB_HEADER_HEIGHT,
                )
            {
                return true;
            }

            if !matches!(ev, Event::KeyDown) {
                return false;
            }

            let key = app::event_key();
            if !matches!(key, Key::Left | Key::Right | Key::Up | Key::Down) {
                return false;
            }

            let children: Vec<Group> = tabs_for_key
                .clone()
                .into_iter()
                .filter_map(|w| w.as_group())
                .collect();
            if children.is_empty() {
                return true;
            }

            let current_ptr = tabs_for_key.value().map(|w| w.as_widget_ptr());
            let index = current_ptr
                .and_then(|ptr| children.iter().position(|g| g.as_widget_ptr() == ptr))
                .unwrap_or(0);

            match key {
                Key::Left | Key::Up => index == 0,
                Key::Right | Key::Down => index + 1 >= children.len(),
                _ => false,
            }
        });

        let suppress_pointer_for_data_cb = suppress_pointer_event_depth.clone();
        let data_tabs_for_key = data_tabs.clone();
        let data_tab_strip_state_for_handle = data_tab_strip_state.clone();
        let data_tab_strip_pointer_gesture =
            Arc::new(Mutex::new(tab_strip::TabStripPointerGesture::default()));
        data_tabs.super_handle_first(false);
        data_tabs.handle(move |tabs, ev| {
            // An active native-first header gesture must always be finalized,
            // even while a nested close/select operation suppresses pointer
            // events. Otherwise FLTK remains native-first and a later wheel or
            // drag can mutate the private tab offset before this filter runs.
            if matches!(
                ev,
                Event::Drag
                    | Event::Released
                    | Event::Unfocus
                    | Event::Deactivate
                    | Event::Hide
                    | Event::MouseWheel
            ) {
                let handled =
                    tab_strip::try_with_state(&data_tab_strip_state_for_handle, |state| {
                        let mut gesture = data_tab_strip_pointer_gesture
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        tab_strip::handle_pointer_event(
                            tabs,
                            ev,
                            state,
                            &mut gesture,
                            constants::TAB_HEADER_HEIGHT,
                        )
                    });
                if handled == Some(true)
                    || (handled.is_none()
                        && ev == Event::MouseWheel
                        && tab_strip::should_consume_mouse_wheel_for_tabs(
                            tabs,
                            constants::TAB_HEADER_HEIGHT,
                        ))
                {
                    return true;
                }
            }
            if Self::should_suppress_pointer_event(&suppress_pointer_for_data_cb, ev) {
                return true;
            }
            if ev == Event::Push
                && tab_strip::try_with_state(&data_tab_strip_state_for_handle, |state| {
                    let mut gesture = data_tab_strip_pointer_gesture
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    tab_strip::handle_pointer_event(
                        tabs,
                        ev,
                        state,
                        &mut gesture,
                        constants::TAB_HEADER_HEIGHT,
                    )
                }) == Some(true)
            {
                return true;
            }
            if Self::should_consume_empty_tab_pointer_event(tabs.children(), ev) {
                return true;
            }
            if !matches!(ev, Event::KeyDown) {
                return false;
            }
            let key = app::event_key();
            if !matches!(key, Key::Left | Key::Right | Key::Up | Key::Down) {
                return false;
            }
            let children: Vec<Group> = data_tabs_for_key
                .clone()
                .into_iter()
                .filter_map(|w| w.as_group())
                .collect();
            if children.is_empty() {
                return true;
            }
            let current_ptr = data_tabs_for_key.value().map(|w| w.as_widget_ptr());
            let index = current_ptr
                .and_then(|ptr| children.iter().position(|g| g.as_widget_ptr() == ptr))
                .unwrap_or(0);
            match key {
                Key::Left | Key::Up => index == 0,
                Key::Right | Key::Down => index + 1 >= children.len(),
                _ => false,
            }
        });

        let mut data_tabs_for_resize = data_tabs.clone();
        let data_tab_strip_state_for_resize = data_tab_strip_state.clone();
        data_tabs.resize_callback(move |t, _, _, _, _| {
            Self::layout_active_tab_child(t);
            Self::sync_data_tab_strip_overflow_mode_for(t, &data_tab_strip_state_for_resize);
        });
        let script_output_for_resize = script_output.clone();
        let script_errors_for_resize = script_errors.clone();
        script_tabs.resize_callback(move |t, _, _, _, _| {
            Self::layout_children(t);
            for pane in [&script_output_for_resize, &script_errors_for_resize] {
                let mut pane = pane.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                Self::layout_text_pane_in_tabs(t, &mut pane);
            }
        });
        let messages_info_for_resize = messages_info.clone();
        let messages_errors_for_resize = messages_errors.clone();
        messages_tabs.resize_callback(move |t, _, _, _, _| {
            Self::layout_children(t);
            for pane in [&messages_info_for_resize, &messages_errors_for_resize] {
                let mut pane = pane.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                Self::layout_text_pane_in_tabs(t, &mut pane);
            }
        });

        let sections_for_resize = sections.clone();
        let mut script_tabs_for_resize = script_tabs.clone();
        let mut messages_tabs_for_resize = messages_tabs.clone();
        let dbms_output_for_resize = dbms_output.clone();
        tabs.resize_callback(move |t, _, _, _, _| {
            Self::layout_children(t);
            Self::layout_active_inner_tabs(
                &sections_for_resize.data_grid,
                &mut data_tabs_for_resize,
            );
            Self::layout_inner_tabs(
                &sections_for_resize.script_output,
                &mut script_tabs_for_resize,
            );
            Self::layout_inner_tabs(&sections_for_resize.messages, &mut messages_tabs_for_resize);
            {
                let mut pane = dbms_output_for_resize
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                Self::layout_text_pane_in_group(&sections_for_resize.dbms_output, &mut pane);
            }
        });

        Self {
            tabs,
            data_tabs,
            script_tabs,
            messages_tabs,
            sections,
            data,
            active_index,
            script_output,
            script_errors,
            dbms_output,
            messages_info,
            messages_errors,
            next_result_tab_id,
            font_profile,
            font_size,
            max_cell_display_chars,
            execute_sql_callback,
            execute_edit_callback,
            lazy_fetch_callback,
            context_action_callback,
            table_browse_callback,
            on_change_callback,
            on_close_callback,
            suppress_pointer_event_depth,
            data_tab_strip_state,
            execution_origin,
        }
    }

    pub(crate) fn set_execution_origin(&mut self, origin: Option<ExecutionOrigin>) {
        *self
            .execution_origin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = origin;
    }

    pub(crate) fn active_result_origin(&self) -> Option<ExecutionOrigin> {
        let active_index = *self
            .active_index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        active_index.and_then(|index| {
            self.data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(index)
                .and_then(|tab| tab.origin.clone())
        })
    }

    pub fn set_on_change<F>(&mut self, callback: F)
    where
        F: FnMut() + 'static,
    {
        *self
            .on_change_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub(crate) fn set_on_close<F>(&mut self, callback: F)
    where
        F: FnMut(ResultTabCloseTarget) + 'static,
    {
        *self
            .on_close_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn get_widget(&self) -> Tabs {
        self.tabs.clone()
    }

    pub fn apply_font_settings(&mut self, profile: FontProfile, size: u32) {
        let size = crate::utils::AppConfig::clamp_font_size(size);
        *self
            .font_profile
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = profile;
        *self
            .font_size
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = size;
        self.with_text_panes(|pane| {
            let mut pane = pane.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            pane.display.set_text_font(profile.normal);
            pane.display.set_text_size(size as i32);
            pane.display.redraw();
        });
        let tables = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|tab| tab.table.clone())
            .collect::<Vec<_>>();
        for mut table in tables {
            table.apply_font_settings(profile, size);
        }
        self.sync_data_tab_strip_overflow_mode();
        self.data_tabs.redraw();
    }

    pub(crate) fn refresh_tab_strip_overflow_mode(&mut self) {
        if self.data_tabs.was_deleted() {
            return;
        }
        self.sync_data_tab_strip_overflow_mode();
        self.data_tabs.redraw();
    }

    pub fn set_max_cell_display_chars(&mut self, max_chars: usize) {
        *self
            .max_cell_display_chars
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = max_chars;
    }

    pub fn clear_grids(&mut self) {
        let _pointer_suppress_guard =
            PointerEventSuppressGuard::new(self.suppress_pointer_event_depth.clone());
        let tabs_to_delete: Vec<_> = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain(..)
            .collect();
        for tab in tabs_to_delete {
            self.delete_tab(tab);
        }
        {
            let mut data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::maybe_shrink_tab_storage(&mut data);
        }
        *self
            .active_index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.sync_data_tab_strip_overflow_mode();
        self.data_tabs.redraw();
        self.fire_on_change_callback();
    }

    pub fn clear(&mut self) {
        self.clear_grids();
        self.clear_support_panes();
        self.fire_on_change_callback();
    }

    pub(crate) fn delete_workspace(&mut self) {
        *self
            .execute_sql_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .lazy_fetch_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .context_action_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .table_browse_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .on_change_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .on_close_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.clear();
        let tabs = self.tabs.clone();
        if !tabs.was_deleted() {
            fltk::group::Tabs::delete(tabs);
        }
    }

    pub fn tab_count(&self) -> usize {
        self.data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    pub fn lazy_fetch_sessions(&self) -> Vec<u64> {
        self.data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter_map(|tab| tab.table.active_lazy_fetch_session())
            .collect()
    }

    pub(crate) fn lazy_fetch_session_for_id(&self, id: ResultTabId) -> Option<u64> {
        self.data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|tab| tab.id == id)
            .and_then(|tab| tab.table.active_lazy_fetch_session())
    }

    pub(crate) fn active_result_id(&self) -> Option<ResultTabId> {
        if !self.top_group_is_current(&self.sections.data_grid) {
            return None;
        }
        let index = *self
            .active_index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        index
            .and_then(|idx| {
                self.data
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get(idx)
                    .map(|tab| (tab.id, tab.group.clone()))
            })
            .and_then(|(id, group)| {
                self.data_tabs
                    .value()
                    .is_some_and(|current| current.as_widget_ptr() == group.as_widget_ptr())
                    .then_some(id)
            })
    }

    pub(crate) fn result_tab_index_for_id(&self, id: ResultTabId) -> Option<usize> {
        self.data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .position(|tab| tab.id == id)
    }

    pub(crate) fn reserve_result_tab_id(&self) -> ResultTabId {
        let mut next_id = self
            .next_result_tab_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let id = ResultTabId::new(*next_id);
        *next_id = next_id.saturating_add(1).max(1);
        id
    }

    pub fn append_script_output_lines(&mut self, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        Self::append_lines_to_pane(&self.script_output, lines);
    }

    pub fn append_dbms_output_lines(&mut self, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        Self::append_lines_to_pane(&self.dbms_output, lines);
    }

    pub fn append_message_lines(&mut self, kind: ResultMessageKind, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        let pane = match kind {
            ResultMessageKind::Info => &self.messages_info,
            ResultMessageKind::Error => &self.messages_errors,
        };
        Self::append_lines_to_pane(pane, lines);
    }

    fn text_result(sql: &str, text: &str, message: &str) -> QueryResult {
        let rows = if text.is_empty() {
            Vec::new()
        } else {
            text.lines().map(|line| vec![line.to_string()]).collect()
        };
        QueryResult {
            sql: sql.to_string(),
            columns: vec![ColumnInfo {
                name: "Text".to_string(),
                data_type: "VARCHAR2".to_string(),
                kind: crate::db::SqlValueKind::Unknown,
            }],
            row_count: rows.len(),
            rows,
            execution_time: Duration::ZERO,
            message: message.to_string(),
            is_select: true,
            success: true,
        }
    }

    pub fn append_explain_plan_tab(&mut self, text: &str) {
        let tab_id = self.reserve_result_tab_id();
        self.ensure_statement_tab_by_id(tab_id, "Explain Plan", true);
        let result = Self::text_result("Explain Plan", text, "Explain plan loaded");
        self.display_result_by_id(tab_id, &result);
    }

    fn start_statement_with_selection(
        &mut self,
        index: usize,
        label: &str,
        select_tab: bool,
        id: Option<ResultTabId>,
    ) {
        let _pointer_suppress_guard =
            PointerEventSuppressGuard::new(self.suppress_pointer_event_depth.clone());
        let origin = self
            .execution_origin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        // Keep the typed origin as metadata for edit/routing safety, but do not
        // put database information in the result-tab header. The header's
        // primary job is to show the live execution state.
        let title = label.trim().to_string();
        if let Some(tab) = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get_mut(index)
        {
            tab.origin = origin.clone();
            if matches!(tab.kind, ResultTabKind::Query) {
                tab.title = title.clone();
            }
        }
        let existing_group = self
            .set_result_tab_state(index, ResultTabStatus::Running, 0)
            .map(|(group, _)| group);
        if let Some(group) = existing_group {
            // Extract the group before calling set_value to avoid re-entrant borrow
            // when the tabs callback fires
            if select_tab {
                self.select_top_group(&self.sections.data_grid.clone());
                let _ = self.data_tabs.set_value(&group);
                Self::record_data_tab_strip_selection_for(
                    &mut self.data_tabs,
                    &self.data_tab_strip_state,
                );
                *self
                    .active_index
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(index);
            }
            return;
        }

        self.data_tabs.begin();
        // Use explicit tab content bounds to avoid relying on hard-coded header height.
        let (x, y, w, h) = Self::content_bounds(&self.data_tabs);
        let mut group = Group::new(x, y, w, h, None).with_label(&Self::result_tab_label_for_title(
            &title,
            index,
            ResultTabStatus::Running,
            0,
        ));
        group.set_color(theme::panel_bg());
        group.set_selection_color(theme::panel_bg());
        group.set_label_color(theme::text_secondary());
        group.set_align(Align::Center | Align::Inside);
        group.set_trigger(CallbackTrigger::Closed);
        let id = id.unwrap_or_else(|| self.reserve_result_tab_id());
        let group_ptr = group.as_widget_ptr() as usize;
        let data_for_close = self.data.clone();
        let on_close_for_group = self.on_close_callback.clone();
        group.set_callback(move |_| {
            if app::callback_reason() != CallbackReason::Closed {
                return;
            }
            let tab_id = data_for_close
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .find_map(|tab| {
                    if tab.group.as_widget_ptr() as usize == group_ptr {
                        Some(tab.id)
                    } else {
                        None
                    }
                });
            if let Some(tab_id) = tab_id {
                Self::fire_on_close_with(&on_close_for_group, ResultTabCloseTarget::Result(tab_id));
            }
        });

        group.begin();
        let mut table = ResultTableWidget::with_size(x, y, w, h);
        table.apply_font_settings(
            *self
                .font_profile
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            *self
                .font_size
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        table.set_max_cell_display_chars(
            *self
                .max_cell_display_chars
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        let execute_sql_callback = self
            .execute_sql_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        table.set_execute_sql_callback(execute_sql_callback);
        let execute_edit_callback = self
            .execute_edit_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        table.set_execute_edit_callback(execute_edit_callback);
        table.set_lazy_fetch_callback(self.lazy_fetch_callback.clone());
        table.set_context_action_callback(self.context_action_callback.clone());
        let widget = table.get_widget();
        group.resizable(&widget);
        group.end();
        self.data_tabs.end();

        let (new_index, new_group) = {
            let mut data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            data.push(ResultTab {
                id,
                title,
                group,
                table,
                status: ResultTabStatus::Running,
                row_count: 0,
                origin,
                kind: ResultTabKind::Query,
                filter_bar: None,
            });
            let idx = data.len().saturating_sub(1);
            let group = data.get(idx).map(|tab| tab.group.clone());
            (idx, group)
        };
        // Extract the group before calling set_value to avoid re-entrant borrow
        // when the tabs callback fires
        if select_tab {
            if let Some(group) = new_group {
                self.select_top_group(&self.sections.data_grid.clone());
                let _ = self.data_tabs.set_value(&group);
            }
            *self
                .active_index
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(new_index);
        }
        self.sync_data_tab_strip_overflow_mode();
        self.fire_on_change_callback();
    }

    pub(crate) fn ensure_statement_tab_by_id(
        &mut self,
        id: ResultTabId,
        label: &str,
        select_tab: bool,
    ) -> Option<usize> {
        if let Some(index) = self.result_tab_index_for_id(id) {
            self.start_statement_with_selection(index, label, select_tab, Some(id));
            return Some(index);
        }

        let index = self.tab_count();
        self.start_statement_with_selection(index, label, select_tab, Some(id));
        self.result_tab_index_for_id(id)
    }

    pub(crate) fn ensure_table_browse_tab_by_id(
        &mut self,
        id: ResultTabId,
        target: TableBrowseTarget,
        intellisense_data: Arc<Mutex<crate::ui::IntellisenseData>>,
        page_size: usize,
        select_tab: bool,
    ) -> Option<usize> {
        let label = target.table_name.clone();
        let index = self.ensure_statement_tab_by_id(id, &label, select_tab)?;
        let already_configured = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(index)
            .is_some_and(|tab| matches!(tab.kind, ResultTabKind::TableBrowse(_)));
        if already_configured {
            if select_tab {
                let filter_bar = {
                    let data = self
                        .data
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    data.get(index).and_then(|tab| tab.filter_bar.clone())
                };
                if let Some(filter_bar) = filter_bar {
                    filter_bar.focus_where_input();
                }
            }
            return Some(index);
        }

        let (group, table) = {
            let data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let tab = data.get(index)?;
            (tab.group.clone(), tab.table.clone())
        };
        let mut request = TableBrowsePageRequest::first(id, target.clone());
        if page_size > 0 {
            request.page_size = page_size;
        }
        let filter_bar = self.build_filter_bar(&group, &table, id, target, intellisense_data);

        {
            let mut data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(tab) = data.get_mut(index) {
                tab.title = label;
                tab.kind = ResultTabKind::TableBrowse(Box::new(TableBrowseState {
                    applied_request: request,
                    pending_request: None,
                    has_next: false,
                    loading: false,
                    last_success: None,
                    last_edit_descriptor: None,
                }));
                tab.filter_bar = Some(filter_bar);
            }
        }
        if let Some(filter_bar) = {
            self.data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(index)
                .and_then(|tab| tab.filter_bar.clone())
        } {
            filter_bar.focus_where_input();
        }
        let _ = self.set_result_tab_state(index, ResultTabStatus::Running, 0);
        Some(index)
    }

    /// Put a filter bar above a tab's grid and shrink the grid to fit.
    ///
    /// Shared by table browsing, which starts life with the bar, and by a
    /// finished query result the user asks to filter.
    fn build_filter_bar(
        &self,
        group: &Group,
        table: &ResultTableWidget,
        id: ResultTabId,
        target: TableBrowseTarget,
        intellisense_data: Arc<Mutex<crate::ui::IntellisenseData>>,
    ) -> TableBrowseFilterBar {
        let mut group = group.clone();
        let (x, y, w, h) = (group.x(), group.y(), group.w(), group.h());
        group.begin();
        let filter_bar = TableBrowseFilterBar::new(
            x,
            y,
            w,
            target,
            intellisense_data,
            id,
            self.table_browse_callback.clone(),
        );
        group.end();

        let mut table_widget = table.get_widget();
        table_widget.resize(
            x,
            y + TABLE_BROWSE_FILTER_HEIGHT,
            w,
            (h - TABLE_BROWSE_FILTER_HEIGHT).max(1),
        );
        group.resizable(&table_widget);
        let mut filter_for_resize = filter_bar.clone();
        let mut table_for_resize = table_widget;
        group.resize_callback(move |group, x, y, w, h| {
            filter_for_resize.layout(x, y, w);
            table_for_resize.resize(
                x,
                y + TABLE_BROWSE_FILTER_HEIGHT,
                w,
                (h - TABLE_BROWSE_FILTER_HEIGHT).max(1),
            );
            group.redraw();
        });
        filter_bar
    }

    /// Give a finished query result a `WHERE` / `ORDER BY` bar, leaving the rows
    /// it is already showing in place.
    ///
    /// The tab deliberately stays a `Query` tab. Result routing and
    /// `execute_table_browse_request` both branch on the tab being a table
    /// browse, so a tab that merely *offers* a filter must not claim to be one
    /// — a statement result arriving in a tab already marked as browsing would
    /// take the page-load path it was never meant to. The tab converts only when
    /// a filter is actually applied, which is the moment a page query really is
    /// in flight (see `promote_query_tab_to_table_browse`).
    ///
    /// `focus_input` belongs to a deliberate request to filter. A bar that
    /// simply appears with a finished result must not take focus — the caret
    /// stays in the editor the user is still typing in.
    ///
    /// Returns false when the tab is gone or already has a bar.
    pub(crate) fn attach_result_filter_bar_by_id(
        &mut self,
        id: ResultTabId,
        target: TableBrowseTarget,
        intellisense_data: Arc<Mutex<crate::ui::IntellisenseData>>,
        focus_input: bool,
    ) -> bool {
        let Some(index) = self.result_tab_index_for_id(id) else {
            return false;
        };
        let parts = {
            let data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(tab) = data.get(index) else {
                return false;
            };
            if tab.filter_bar.is_some() || !matches!(tab.kind, ResultTabKind::Query) {
                return false;
            }
            (tab.group.clone(), tab.table.clone())
        };
        let filter_bar =
            self.build_filter_bar(&parts.0, &parts.1, id, target.clone(), intellisense_data);
        {
            let mut data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(tab) = data.get_mut(index) {
                tab.filter_bar = Some(filter_bar.clone());
            }
        }
        parts.0.clone().redraw();
        if focus_input {
            filter_bar.focus_where_input();
        }
        true
    }

    /// Whether this tab carries a filter bar, however it got one.
    ///
    /// The header sort asks this: a tab that can re-query orders on the server
    /// instead of sorting the fetched rows locally.
    pub(crate) fn result_tab_has_filter_bar(&self, id: ResultTabId) -> bool {
        self.result_tab_index_for_id(id).is_some_and(|index| {
            self.data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(index)
                .is_some_and(|tab| tab.filter_bar.is_some())
        })
    }

    pub(crate) fn set_table_browse_callback(&mut self, callback: TableBrowseExecuteCallback) {
        let mut callback_fn = callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        *self
            .table_browse_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = callback_fn.take();
    }

    pub(crate) fn is_table_browse_tab(&self, id: ResultTabId) -> bool {
        self.result_tab_index_for_id(id).is_some_and(|index| {
            self.data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(index)
                .is_some_and(|tab| matches!(tab.kind, ResultTabKind::TableBrowse(_)))
        })
    }

    pub(crate) fn table_browse_is_loading(&self, id: ResultTabId) -> bool {
        self.result_tab_index_for_id(id).is_some_and(|index| {
            self.data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(index)
                .is_some_and(
                    |tab| matches!(&tab.kind, ResultTabKind::TableBrowse(state) if state.loading),
                )
        })
    }

    pub(crate) fn table_browse_applied_request(
        &self,
        id: ResultTabId,
    ) -> Option<TableBrowsePageRequest> {
        let index = self.result_tab_index_for_id(id)?;
        let data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let ResultTabKind::TableBrowse(state) = &data.get(index)?.kind else {
            return None;
        };
        Some(state.applied_request.clone())
    }

    pub(crate) fn current_table_browse_page_size(&self) -> Option<usize> {
        let index = (*self
            .active_index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))?;
        let data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let ResultTabKind::TableBrowse(state) = &data.get(index)?.kind else {
            return None;
        };
        Some(state.applied_request.page_size)
    }

    pub(crate) fn capture_table_browse_current_page(&mut self, id: ResultTabId) -> bool {
        let Some(index) = self.result_tab_index_for_id(id) else {
            return false;
        };
        let mut data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(tab) = data.get_mut(index) else {
            return false;
        };
        if !matches!(&tab.kind, ResultTabKind::TableBrowse(_)) {
            return false;
        }
        let snapshot = tab.table.snapshot_select_result();
        let descriptor = tab.table.result_edit_descriptor_snapshot();
        let ResultTabKind::TableBrowse(state) = &mut tab.kind else {
            return false;
        };
        state.last_success = Some(snapshot);
        if descriptor.is_some() {
            state.last_edit_descriptor = descriptor;
        }
        true
    }

    pub(crate) fn normalize_table_browse_request(
        &self,
        request: &mut TableBrowsePageRequest,
    ) -> bool {
        let Some(index) = self.result_tab_index_for_id(request.result_tab_id) else {
            return false;
        };
        let data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(ResultTabKind::TableBrowse(state)) = data.get(index).map(|tab| &tab.kind) else {
            return false;
        };
        state.normalize_request(request);
        true
    }

    pub(crate) fn begin_table_browse_request(
        &mut self,
        request: TableBrowsePageRequest,
    ) -> Result<(), String> {
        let Some(index) = self.result_tab_index_for_id(request.result_tab_id) else {
            return Err("The table result tab is closed.".to_string());
        };
        let (row_count, mut filter_bar) = {
            let mut data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(tab) = data.get_mut(index) else {
                return Err("The table result tab is closed.".to_string());
            };
            let ResultTabKind::TableBrowse(state) = &mut tab.kind else {
                return Err("The active result is not a table browse tab.".to_string());
            };
            state.pending_request = Some(request);
            state.loading = true;
            (tab.row_count, tab.filter_bar.clone())
        };
        if let Some(filter_bar) = filter_bar.as_mut() {
            filter_bar.set_active(false);
        }
        let _ = self.set_result_tab_state(index, ResultTabStatus::Running, row_count);
        Ok(())
    }

    pub(crate) fn table_browse_initial_request(
        &self,
        id: ResultTabId,
    ) -> Option<TableBrowsePageRequest> {
        let index = self.result_tab_index_for_id(id)?;
        let data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let ResultTabKind::TableBrowse(state) = &data.get(index)?.kind else {
            return None;
        };
        Some(state.applied_request.clone())
    }

    fn start_streaming(
        &mut self,
        index: usize,
        columns: &[String],
        column_kinds: &[SqlValueKind],
        null_text: &str,
        sql: &str,
    ) {
        let status = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(index)
            .map(|tab| ResultTabStatus::for_stream_update(tab.status));
        let table = status
            .and_then(|status| self.set_result_tab_state(index, status, 0))
            .map(|(_, table)| table);
        if let Some(mut table) = table {
            table.set_null_text(null_text);
            table.start_streaming(columns);
            // Must follow start_streaming: the setter validates the kinds
            // against the header list that call just installed, and
            // start_streaming clears the statement text this reinstalls.
            table.set_column_kinds(column_kinds);
            table.set_streaming_source_sql(sql);
        }
        self.fire_on_change_callback();
    }

    pub(crate) fn start_streaming_by_id(
        &mut self,
        id: ResultTabId,
        columns: &[String],
        column_kinds: &[SqlValueKind],
        null_text: &str,
        sql: &str,
    ) {
        if let Some(index) = self.result_tab_index_for_id(id) {
            self.start_streaming(index, columns, column_kinds, null_text, sql);
        }
    }

    pub(crate) fn set_result_edit_descriptor_by_id(
        &mut self,
        id: ResultTabId,
        descriptor: ResultEditDescriptor,
    ) {
        let table = self.result_tab_index_for_id(id).and_then(|index| {
            self.data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(index)
                .map(|tab| tab.table.clone())
        });
        if let Some(mut table) = table {
            table.set_result_edit_descriptor(descriptor);
        }
    }

    fn append_rows(&mut self, index: usize, rows: Vec<Vec<String>>) {
        let rows_len = rows.len();
        let table = {
            let data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            data.get(index).map(|tab| {
                (
                    tab.row_count.saturating_add(rows_len),
                    ResultTabStatus::for_stream_update(tab.status),
                    tab.table.clone(),
                )
            })
        }
        .and_then(|(row_count, status, table)| {
            self.set_result_tab_state(index, status, row_count)
                .map(|_| table)
        });
        if let Some(mut table) = table {
            table.append_rows(rows);
        }
    }

    pub(crate) fn append_rows_by_id(&mut self, id: ResultTabId, rows: Vec<Vec<String>>) {
        if let Some(index) = self.result_tab_index_for_id(id) {
            self.append_rows(index, rows);
        }
    }

    fn finish_streaming(&mut self, index: usize) {
        let table = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(index)
            .map(|tab| tab.table.clone());
        if let Some(mut table) = table {
            table.finish_streaming();
        }
        self.fire_on_change_callback();
    }

    pub(crate) fn finish_streaming_by_id(&mut self, id: ResultTabId) {
        if let Some(index) = self.result_tab_index_for_id(id) {
            self.finish_streaming(index);
        }
    }

    fn set_lazy_fetch_session(&mut self, index: usize, session_id: u64) {
        let table = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(index)
            .map(|tab| tab.table.clone());
        if let Some(mut table) = table {
            table.set_lazy_fetch_session(session_id);
        }
        self.fire_on_change_callback();
    }

    pub(crate) fn set_lazy_fetch_session_by_id(&mut self, id: ResultTabId, session_id: u64) {
        if let Some(index) = self.result_tab_index_for_id(id) {
            self.set_lazy_fetch_session(index, session_id);
        }
    }

    /// Tell a result tab's grid where the backend that owns it puts NULLs on an
    /// ascending sort, so the local header sort agrees with the server.
    ///
    /// Set on the tab rather than on a streaming batch: it belongs to the
    /// connection, and every result the tab goes on to show comes from the same
    /// one.
    fn mark_lazy_fetch_waiting(&mut self, index: usize, session_id: u64) {
        let tab_parts = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(index)
            .map(|tab| (tab.row_count, tab.table.clone()));
        if let Some((row_count, mut table)) = tab_parts {
            let continued_page_fetch = table.note_lazy_fetch_waiting(session_id);
            let status = if continued_page_fetch {
                ResultTabStatus::Fetching
            } else {
                ResultTabStatus::Waiting
            };
            self.set_result_tab_state(index, status, row_count);
        }
        self.fire_on_change_callback();
    }

    pub(crate) fn mark_lazy_fetch_waiting_by_id(&mut self, id: ResultTabId, session_id: u64) {
        if let Some(index) = self.result_tab_index_for_id(id) {
            self.mark_lazy_fetch_waiting(index, session_id);
        }
    }

    fn mark_statement_canceling(&mut self, index: usize) {
        self.mark_statement_status(index, ResultTabStatus::Canceling);
    }

    pub(crate) fn mark_statement_canceling_by_id(&mut self, id: ResultTabId) {
        if let Some(index) = self.result_tab_index_for_id(id) {
            self.mark_statement_canceling(index);
        }
    }

    fn mark_statement_cancelled(&mut self, index: usize) {
        self.mark_statement_status(index, ResultTabStatus::Cancelled);
    }

    pub(crate) fn mark_statement_cancelled_by_id(&mut self, id: ResultTabId) {
        if let Some(index) = self.result_tab_index_for_id(id) {
            self.mark_statement_cancelled(index);
        }
    }

    pub(crate) fn mark_statement_status_by_id(&mut self, id: ResultTabId, status: ResultTabStatus) {
        if let Some(index) = self.result_tab_index_for_id(id) {
            self.mark_statement_status(index, status);
        }
    }

    fn mark_statement_status(&mut self, index: usize, status: ResultTabStatus) {
        let row_count = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(index)
            .map(|tab| tab.row_count);
        if let Some(row_count) = row_count {
            self.set_result_tab_state(index, status, row_count);
        }
        self.fire_on_change_callback();
    }

    pub fn mark_lazy_fetch_canceling(&mut self, session_id: u64) -> bool {
        let tab_updates: Vec<(usize, usize)> = {
            let data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            data.iter()
                .enumerate()
                .filter_map(|(index, tab)| {
                    if tab.table.active_lazy_fetch_session() == Some(session_id) {
                        Some((index, tab.row_count))
                    } else {
                        None
                    }
                })
                .collect()
        };
        if tab_updates.is_empty() {
            return false;
        }
        for (index, row_count) in tab_updates {
            self.set_result_tab_state(index, ResultTabStatus::Canceling, row_count);
        }
        self.fire_on_change_callback();
        true
    }

    fn clear_lazy_fetch_session(&mut self, index: usize, session_id: u64, run_pending: bool) {
        let tab_parts = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(index)
            .map(|tab| (tab.row_count, tab.table.clone()));
        let table = if let Some((row_count, table)) = tab_parts {
            self.set_result_tab_state(index, ResultTabStatus::Done, row_count);
            Some(table)
        } else {
            None
        };
        if let Some(mut table) = table {
            table.clear_lazy_fetch_session(session_id, run_pending);
        }
        self.fire_on_change_callback();
    }

    pub(crate) fn clear_lazy_fetch_session_by_id(
        &mut self,
        id: ResultTabId,
        session_id: u64,
        run_pending: bool,
    ) {
        if let Some(index) = self.result_tab_index_for_id(id) {
            self.clear_lazy_fetch_session(index, session_id, run_pending);
        }
    }

    pub fn abort_lazy_fetch_session(&mut self, session_id: u64) -> bool {
        let tab_updates: Vec<(usize, ResultTabStatus, usize, ResultTableWidget)> = {
            let data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            data.iter()
                .enumerate()
                .filter_map(|(index, tab)| {
                    if tab.table.active_lazy_fetch_session() == Some(session_id) {
                        let status = if tab.status == ResultTabStatus::Error {
                            ResultTabStatus::Error
                        } else {
                            ResultTabStatus::Cancelled
                        };
                        Some((index, status, tab.row_count, tab.table.clone()))
                    } else {
                        None
                    }
                })
                .collect()
        };
        if tab_updates.is_empty() {
            return false;
        }
        let mut tables = Vec::with_capacity(tab_updates.len());
        for (index, status, row_count, table) in tab_updates {
            self.set_result_tab_state(index, status, row_count);
            tables.push(table);
        }
        for mut table in tables {
            table.clear_lazy_fetch_session(session_id, false);
            table.finish_streaming();
        }
        self.fire_on_change_callback();
        true
    }

    pub fn finish_all_streaming(&mut self) {
        let tables: Vec<ResultTableWidget> = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|tab| tab.table.clone())
            .collect();
        for mut table in tables {
            table.finish_streaming();
        }
    }

    pub fn finish_non_lazy_streaming(&mut self) {
        let tables: Vec<ResultTableWidget> = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter_map(|tab| {
                if tab.table.active_lazy_fetch_session().is_none() {
                    Some(tab.table.clone())
                } else {
                    None
                }
            })
            .collect();
        for mut table in tables {
            table.finish_streaming();
        }
    }

    pub fn clear_all_lazy_fetch_state_for_abort(&mut self) {
        let tables: Vec<ResultTableWidget> = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|tab| tab.table.clone())
            .collect();
        for mut table in tables {
            table.clear_lazy_fetch_state_for_abort();
        }
        self.fire_on_change_callback();
    }

    pub fn clear_orphaned_save_requests(&mut self) -> usize {
        let tables: Vec<ResultTableWidget> = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|tab| tab.table.clone())
            .collect();
        let mut cleared = 0usize;
        for mut table in tables {
            if table.clear_orphaned_save_request() {
                cleared = cleared.saturating_add(1);
            }
        }
        if cleared > 0 {
            self.fire_on_change_callback();
        }
        cleared
    }

    pub fn clear_orphaned_query_edit_backups(&mut self) -> usize {
        let tables: Vec<ResultTableWidget> = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|tab| tab.table.clone())
            .collect();
        let mut restored = 0usize;
        for mut table in tables {
            if table.clear_orphaned_query_edit_backup() {
                restored = restored.saturating_add(1);
            }
        }
        if restored > 0 {
            self.fire_on_change_callback();
        }
        restored
    }

    fn display_result(&mut self, index: usize, result: &crate::db::QueryResult) {
        let is_table_browse = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(index)
            .is_some_and(|tab| matches!(tab.kind, ResultTabKind::TableBrowse(_)));
        if is_table_browse {
            self.display_table_browse_result(index, result);
            return;
        }
        let status = ResultTabStatus::from_query_result(result);
        let table = self
            .set_result_tab_state(index, status, result.row_count)
            .map(|(_, table)| table);
        if let Some(table) = table {
            let mut table = table;
            table.display_result(result);
        }
        self.fire_on_change_callback();
    }

    fn display_table_browse_result(&mut self, index: usize, result: &QueryResult) {
        if !result.success || !result.is_select {
            self.fail_table_browse_result(index);
            return;
        }
        let (request, mut table) = {
            let data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(tab) = data.get(index) else {
                return;
            };
            let ResultTabKind::TableBrowse(state) = &tab.kind else {
                return;
            };
            (
                state
                    .pending_request
                    .clone()
                    .unwrap_or_else(|| state.applied_request.clone()),
                tab.table.clone(),
            )
        };

        table.set_row_number_offset(request.offset);
        table.display_result(result);
        let logical_sql = request.logical_sql().unwrap_or_else(|_| result.sql.clone());
        let (displayed_rows, has_next) = table.postprocess_table_browse_page(
            request.page_size,
            request
                .target
                .db_type
                .table_browse_spec()
                .strips_page_helper_column,
            &logical_sql,
        );
        let snapshot = table.snapshot_select_result();
        let descriptor = table.result_edit_descriptor_snapshot();
        let page_number = Self::table_browse_page_number(request.offset, request.page_size);
        let (mut filter_bar, title, group) = {
            let mut data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(tab) = data.get_mut(index) else {
                return;
            };
            let ResultTabKind::TableBrowse(state) = &mut tab.kind else {
                return;
            };
            state.applied_request = request;
            state.pending_request = None;
            state.has_next = has_next;
            state.loading = false;
            state.last_success = Some(snapshot);
            state.last_edit_descriptor = descriptor;
            tab.status = ResultTabStatus::Done;
            tab.row_count = displayed_rows;
            (tab.filter_bar.clone(), tab.title.clone(), tab.group.clone())
        };
        if let Some(filter_bar) = filter_bar.as_mut() {
            filter_bar.set_active(true);
        }
        self.update_table_browse_tab_group_label(&title, page_number, displayed_rows, group);
        self.fire_on_change_callback();
    }

    fn fail_table_browse_result(&mut self, index: usize) {
        let (last_success, descriptor, row_count, mut table, mut filter_bar) = {
            let mut data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(tab) = data.get_mut(index) else {
                return;
            };
            let ResultTabKind::TableBrowse(state) = &mut tab.kind else {
                return;
            };
            state.pending_request = None;
            state.loading = false;
            let row_count = state
                .last_success
                .as_ref()
                .map(|result| result.rows.len())
                .unwrap_or(tab.row_count);
            (
                state.last_success.clone(),
                state.last_edit_descriptor.clone(),
                row_count,
                tab.table.clone(),
                tab.filter_bar.clone(),
            )
        };
        if let Some(last_success) = last_success {
            table.display_result(&last_success);
            if let Some(descriptor) = descriptor {
                table.set_result_edit_descriptor(descriptor);
            }
        }
        if let Some(filter_bar) = filter_bar.as_mut() {
            filter_bar.set_active(true);
        }
        let _ = self.set_result_tab_state(index, ResultTabStatus::Error, row_count);
        self.fire_on_change_callback();
    }

    pub(crate) fn fail_table_browse_result_by_id(&mut self, id: ResultTabId) {
        if let Some(index) = self.result_tab_index_for_id(id) {
            self.fail_table_browse_result(index);
        }
    }

    pub(crate) fn display_result_by_id(
        &mut self,
        id: ResultTabId,
        result: &crate::db::QueryResult,
    ) {
        if let Some(index) = self.result_tab_index_for_id(id) {
            self.display_result(index, result);
        }
    }

    fn finish_result_status(&mut self, index: usize, result: &crate::db::QueryResult) {
        let status = ResultTabStatus::from_query_result(result);
        let _ = self.set_result_tab_state(index, status, result.row_count);
        self.fire_on_change_callback();
    }

    pub(crate) fn finish_result_status_by_id(
        &mut self,
        id: ResultTabId,
        result: &crate::db::QueryResult,
    ) {
        if let Some(index) = self.result_tab_index_for_id(id) {
            self.finish_result_status(index, result);
        }
    }

    /// Deliver a result-grid edit execution's terminal (DML) result to the
    /// editable tab's table so its pending-save matching runs: clearing the
    /// save on success while keeping the staged rows, or preserving staged
    /// edits on failure. Unlike `display_result` this does not overwrite the
    /// tab's status/row-count badge, because the editable grid keeps showing
    /// the original result set rather than the DML's affected-row summary.
    fn deliver_result_grid_execution_result(
        &mut self,
        index: usize,
        result: &crate::db::QueryResult,
    ) {
        let table = {
            let data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            data.get(index).map(|tab| tab.table.clone())
        };
        if let Some(mut table) = table {
            table.display_result(result);
        }
        self.fire_on_change_callback();
    }

    pub(crate) fn deliver_result_grid_execution_result_by_id(
        &mut self,
        id: ResultTabId,
        result: &crate::db::QueryResult,
    ) {
        if let Some(index) = self.result_tab_index_for_id(id) {
            self.deliver_result_grid_execution_result(index, result);
        }
    }

    pub fn set_execute_sql_callback(&mut self, callback: ResultGridSqlExecuteCallback) {
        *self
            .execute_sql_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(callback.clone());
        let tabs = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for tab in tabs.iter() {
            let mut table = tab.table.clone();
            table.set_execute_sql_callback(Some(callback.clone()));
        }
    }

    pub fn set_execute_edit_callback(&mut self, callback: ResultGridEditExecuteCallback) {
        *self
            .execute_edit_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(callback.clone());
        let tabs = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for tab in tabs.iter() {
            let mut table = tab.table.clone();
            table.set_execute_edit_callback(Some(callback.clone()));
        }
    }

    pub fn set_lazy_fetch_callback(&mut self, callback: LazyFetchCallback) {
        *self
            .lazy_fetch_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Box::new(move |id, request| {
                ResultTabsWidget::invoke_lazy_fetch_callback(&callback, id, request)
            }));
        let tabs = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for tab in tabs.iter() {
            let mut table = tab.table.clone();
            table.set_lazy_fetch_callback(self.lazy_fetch_callback.clone());
        }
    }

    pub fn set_context_action_callback(&mut self, callback: ResultTableContextActionCallback) {
        let mut guard = self
            .context_action_callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some(Box::new(move |action| {
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
        }));
        let tabs = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for tab in tabs.iter() {
            let mut table = tab.table.clone();
            table.set_context_action_callback(self.context_action_callback.clone());
        }
    }

    pub fn export_to_csv(&self) -> String {
        self.current_table()
            .map(|table| table.export_to_csv())
            .unwrap_or_default()
    }

    /// Render the visible grid in `format` and hand the text to `callback`.
    ///
    /// Returns the text directly when it was ready; `None` means a full fetch
    /// had to run first and the callback will fire when it finishes.
    pub(crate) fn export_after_fetch_all(
        &self,
        format: ExportFormat,
        scope: ExportScope,
        destination: ExportDestination,
        db_type: Option<crate::db::DatabaseType>,
        callback: Box<dyn FnMut(String, usize)>,
    ) -> Option<(String, usize)> {
        let table = self.current_table()?;
        let request = ExportRequest {
            format,
            scope,
            destination,
            db_type,
            table: Self::resolve_grid_export_table(&table),
        };
        table.export_after_fetch_all(request, callback)
    }

    /// Whether the visible grid has a selection an export could be narrowed to.
    pub(crate) fn has_grid_selection(&self) -> bool {
        self.current_table()
            .is_some_and(|table| table.has_selection())
    }

    pub fn row_count(&self) -> usize {
        self.current_table()
            .map(|table| table.row_count())
            .unwrap_or(0)
    }

    pub fn has_data(&self) -> bool {
        self.current_table()
            .map(|table| table.has_data())
            .unwrap_or(false)
    }

    pub fn can_current_begin_edit_mode(&self) -> bool {
        self.current_table()
            .map(|table| table.can_begin_edit_mode())
            .unwrap_or(false)
    }

    pub fn is_current_save_pending(&self) -> bool {
        self.current_table()
            .map(|table| table.is_save_pending())
            .unwrap_or(false)
    }

    /// Whether this tab's grid holds unsaved edits.
    pub(crate) fn result_tab_has_staged_edits(&self, id: ResultTabId) -> bool {
        self.result_tab_index_for_id(id).is_some_and(|index| {
            self.data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(index)
                .is_some_and(|tab| tab.table.has_staged_edits())
        })
    }

    pub fn is_current_edit_mode_enabled(&self) -> bool {
        self.current_table()
            .map(|table| table.is_edit_mode_enabled())
            .unwrap_or(false)
    }

    pub fn begin_current_edit_mode(&mut self) -> Result<String, String> {
        let Some(mut table) = self.current_table() else {
            return Err("Open a result tab first.".to_string());
        };
        let result = table.begin_edit_mode();
        self.fire_on_change_callback();
        result
    }

    pub fn insert_row_in_current_edit_mode(&mut self) -> Result<String, String> {
        let Some(mut table) = self.current_table() else {
            return Err("Open a result tab first.".to_string());
        };
        let result = table.insert_row_in_edit_mode();
        self.fire_on_change_callback();
        result
    }

    pub fn delete_selected_rows_in_current_edit_mode(&mut self) -> Result<String, String> {
        let Some(mut table) = self.current_table() else {
            return Err("Open a result tab first.".to_string());
        };
        let result = table.delete_selected_rows_in_edit_mode();
        self.fire_on_change_callback();
        result
    }

    pub fn save_current_edit_mode(&mut self) -> Result<String, String> {
        let Some(mut table) = self.current_table() else {
            return Err("Open a result tab first.".to_string());
        };
        let result = table.save_edit_mode();
        self.fire_on_change_callback();
        result
    }

    pub fn cancel_current_edit_mode(&mut self) -> Result<String, String> {
        let Some(mut table) = self.current_table() else {
            return Err("Open a result tab first.".to_string());
        };
        let result = table.cancel_edit_mode();
        self.fire_on_change_callback();
        result
    }

    fn current_table(&self) -> Option<ResultTableWidget> {
        let index = *self
            .active_index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        index
            .and_then(|idx| {
                self.data
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get(idx)
                    .cloned()
            })
            .map(|tab| tab.table)
    }

    fn navigate_current_page<F>(&mut self, navigate: F) -> bool
    where
        F: FnOnce(&mut ResultTableWidget) -> ResultPageNavigationOutcome,
    {
        let Some(id) = self.active_result_id() else {
            return false;
        };
        let Some(index) = self.result_tab_index_for_id(id) else {
            return false;
        };
        let Some((status, row_count, mut table)) = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(index)
            .map(|tab| (tab.status, tab.row_count, tab.table.clone()))
        else {
            return false;
        };
        let outcome = navigate(&mut table);
        if outcome == ResultPageNavigationOutcome::FetchRequested {
            self.set_result_tab_state(index, ResultTabStatus::for_stream_update(status), row_count);
        }
        if outcome != ResultPageNavigationOutcome::NoChange {
            self.fire_on_change_callback();
            true
        } else {
            false
        }
    }

    fn table_browse_navigation_request<F>(&mut self, build: F) -> Option<TableBrowsePageRequest>
    where
        F: FnOnce(&TableBrowseState) -> Option<TableBrowsePageRequest>,
    {
        let index = (*self
            .active_index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))?;
        let data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tab = data.get(index)?;
        let ResultTabKind::TableBrowse(state) = &tab.kind else {
            return None;
        };
        if state.loading {
            return None;
        }
        build(state)
    }

    fn current_is_table_browse(&self) -> bool {
        let index = *self
            .active_index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        index.is_some_and(|index| {
            self.data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(index)
                .is_some_and(|tab| matches!(tab.kind, ResultTabKind::TableBrowse(_)))
        })
    }

    fn invoke_table_browse_request(&self, request: TableBrowsePageRequest) -> bool {
        match invoke_table_browse_execute_callback(&self.table_browse_callback, request) {
            Ok(()) => true,
            Err(message) => {
                crate::ui::alert_on_main(&message);
                false
            }
        }
    }

    pub(crate) fn page_current_first(&mut self) -> bool {
        let table_browse = self.current_is_table_browse();
        if let Some(request) = self.table_browse_navigation_request(|state| {
            let mut request = state.applied_request.clone();
            request.offset = 0;
            request.navigation = TableBrowseNavigation::Page;
            Some(request)
        }) {
            return self.invoke_table_browse_request(request);
        }
        if table_browse {
            return false;
        }
        self.navigate_current_page(ResultTableWidget::page_first)
    }

    pub(crate) fn page_current_previous(&mut self, unit: usize) -> bool {
        let table_browse = self.current_is_table_browse();
        if let Some(request) = self.table_browse_navigation_request(|state| {
            (state.applied_request.offset > 0).then(|| {
                let mut request = state.applied_request.clone();
                request.offset = request.offset.saturating_sub(request.page_size as u64);
                request.navigation = TableBrowseNavigation::Page;
                request
            })
        }) {
            return self.invoke_table_browse_request(request);
        }
        if table_browse {
            return false;
        }
        self.navigate_current_page(|table| table.page_previous(unit))
    }

    pub(crate) fn page_current_next(&mut self, unit: usize) -> bool {
        let table_browse = self.current_is_table_browse();
        if let Some(request) = self.table_browse_navigation_request(|state| {
            state.has_next.then(|| {
                let mut request = state.applied_request.clone();
                request.offset = request
                    .offset
                    .saturating_add(u64::try_from(request.page_size).unwrap_or(u64::MAX));
                request.navigation = TableBrowseNavigation::Page;
                request
            })
        }) {
            return self.invoke_table_browse_request(request);
        }
        if table_browse {
            return false;
        }
        self.navigate_current_page(|table| table.page_next(unit))
    }

    pub(crate) fn page_current_last(&mut self) -> bool {
        let table_browse = self.current_is_table_browse();
        if let Some(request) = self.table_browse_navigation_request(|state| {
            let mut request = state.applied_request.clone();
            request.navigation = TableBrowseNavigation::Last;
            Some(request)
        }) {
            return self.invoke_table_browse_request(request);
        }
        if table_browse {
            return false;
        }
        self.navigate_current_page(ResultTableWidget::page_last)
    }

    pub(crate) fn set_current_page_unit(&mut self, unit: usize) -> bool {
        if unit == 0 {
            return false;
        }
        let request = {
            let Some(index) = *self
                .active_index
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
            else {
                return false;
            };
            let data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(tab) = data.get(index) else {
                return false;
            };
            let ResultTabKind::TableBrowse(state) = &tab.kind else {
                return false;
            };
            if state.loading || state.applied_request.page_size == unit {
                return false;
            }
            let mut request = state.applied_request.clone();
            request.page_size = unit;
            request.offset = 0;
            request.navigation = TableBrowseNavigation::Page;
            request
        };
        self.invoke_table_browse_request(request)
    }

    pub fn copy(&self) -> usize {
        if let Some(table) = self.current_table() {
            table.copy()
        } else {
            0
        }
    }

    pub fn copy_with_headers(&self) {
        if let Some(table) = self.current_table() {
            table.copy_with_headers();
        }
    }

    pub fn select_all(&self) {
        if let Some(mut table) = self.current_table() {
            table.select_all();
        }
    }

    /// Snapshot the visible grid's selection for SQL export, with its base table
    /// already resolved.
    ///
    /// `source_sql_snapshot` reports the streaming statement while the grid has
    /// no finished result, so a grid that is still fetching — or that a
    /// cancelled lazy fetch left populated — still names its real table.
    pub(crate) fn sql_export_context(
        &self,
        db_type: crate::db::DatabaseType,
    ) -> Option<GridSqlSelection> {
        let table = self.current_table()?;
        let mut selection = table.sql_export_selection(db_type, None)?;
        selection.table = Self::resolve_grid_export_table(&table);
        Some(selection)
    }

    /// The base table generated SQL should name for this grid, if one exists.
    fn resolve_grid_export_table(table: &ResultTableWidget) -> Option<String> {
        let descriptor_table = table
            .result_edit_descriptor_snapshot()
            .map(|descriptor| format!("{}.{}", descriptor.schema_name, descriptor.table_name));
        crate::ui::grid_sql_export::resolve_export_table(
            descriptor_table,
            &table.source_sql_snapshot(),
        )
    }

    #[doc(hidden)]
    pub(crate) fn capture_tour_show_context_menu(&self) -> Result<(), String> {
        self.current_table()
            .ok_or_else(|| "no visible result grid".to_string())?
            .capture_tour_show_context_menu()
    }

    #[doc(hidden)]
    pub(crate) fn capture_tour_select_range(
        &self,
        row_start: i32,
        col_start: i32,
        row_end: i32,
        col_end: i32,
    ) {
        if let Some(mut table) = self.current_table() {
            table.capture_tour_select_range(row_start, col_start, row_end, col_end);
        }
    }

    pub(crate) fn capture_tour_clear_selection(&self) {
        if let Some(mut table) = self.current_table() {
            table.capture_tour_clear_selection();
        }
    }

    pub(crate) fn capture_tour_show_table_browse_popup(
        &mut self,
    ) -> Option<(Input, Arc<Mutex<IntellisensePopup>>)> {
        let index = {
            let active_index = self
                .active_index
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (*active_index)?
        };
        let mut filter_bar = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(index)?
            .filter_bar
            .clone()?;
        Some(filter_bar.capture_tour_show_where_popup())
    }

    pub(crate) fn capture_tour_show_table_browse_order_popup(
        &mut self,
    ) -> Option<(Input, Arc<Mutex<IntellisensePopup>>)> {
        let index = {
            let active_index = self
                .active_index
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (*active_index)?
        };
        let mut filter_bar = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(index)?
            .filter_bar
            .clone()?;
        Some(filter_bar.capture_tour_show_order_popup())
    }

    pub fn paste_from_clipboard(&self) -> bool {
        if let Some(mut table) = self.current_table() {
            table.paste_from_clipboard();
            true
        } else {
            false
        }
    }

    fn delete_tab(&mut self, mut tab: ResultTab) {
        // FLTK memory management: proper cleanup order is critical
        // 1. Clear callbacks on child widgets to release captured Arc<Mutex<T>> references
        // 2. Remove child widgets from parent before deletion
        // 3. Delete child widgets
        // 4. Delete parent container

        Self::record_data_tab_strip_removal_for(&tab.group, &self.data_tab_strip_state);

        let mut group = tab.group;
        if !group.was_deleted() {
            group.resize_callback(|_, _, _, _, _| {});
        }
        if let Some(filter_bar) = tab.filter_bar.as_mut() {
            filter_bar.cleanup_for_close();
        }

        // Step 1: Cleanup the table widget (clears callbacks and data buffers)
        tab.table.cleanup();

        // Step 2 & 3: Explicitly remove/delete the table widget first to ensure
        // callback closures are dropped immediately, then clear/delete any
        // additional child widgets that may be added to result tabs in the future.
        let table_widget = tab.table.get_widget();
        if !group.was_deleted()
            && !table_widget.was_deleted()
            && group.find(&table_widget) < group.children()
        {
            group.remove(&table_widget);
        }
        if !table_widget.was_deleted() {
            fltk::table::Table::delete(table_widget);
        }
        if !group.was_deleted() {
            group.clear();
        }

        // Step 4: Remove group from tabs and delete
        if !self.data_tabs.was_deleted()
            && !group.was_deleted()
            && self.data_tabs.find(&group) < self.data_tabs.children()
        {
            self.data_tabs.remove(&group);
        }
        if !group.was_deleted() {
            fltk::group::Group::delete(group);
        }
    }

    /// Close the currently active result tab, freeing its data and FLTK resources.
    /// Returns true if a tab was closed.
    pub fn close_current_tab(&mut self) -> bool {
        self.close_current_tab_and_take_lazy_fetch().is_some()
    }

    pub fn close_current_script_output_tab(&mut self) -> bool {
        false
    }

    pub fn close_script_output_tab(&mut self) -> bool {
        false
    }

    fn close_current_tab_and_take_lazy_fetch(&mut self) -> Option<(ResultTabId, Option<u64>)> {
        let id = self.active_result_id()?;
        self.close_tab_by_id_and_take_lazy_fetch(id)
    }

    pub(crate) fn close_tab_by_id_and_take_lazy_fetch(
        &mut self,
        id: ResultTabId,
    ) -> Option<(ResultTabId, Option<u64>)> {
        let index = self.result_tab_index_for_id(id)?;
        let (_, lazy_fetch_session) = self.close_tab_at_and_take_lazy_fetch(index)?;
        Some((id, lazy_fetch_session))
    }

    fn close_tab_at_and_take_lazy_fetch(&mut self, index: usize) -> Option<(usize, Option<u64>)> {
        let active_before = *self
            .active_index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _pointer_suppress_guard =
            PointerEventSuppressGuard::new(self.suppress_pointer_event_depth.clone());
        let (tab, selected_result_index, selected_group) = {
            let mut data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if index >= data.len() {
                return None;
            }

            let tab = data.remove(index);
            let remaining = data.len();
            let selected_result_index = active_before.and_then(|active| {
                if remaining == 0 {
                    None
                } else if active == index {
                    Some(index.min(remaining - 1))
                } else if active > index {
                    Some((active - 1).min(remaining - 1))
                } else {
                    Some(active.min(remaining - 1))
                }
            });
            let selected_group = selected_result_index
                .and_then(|selected_index| data.get(selected_index).map(|tab| tab.group.clone()));
            (tab, selected_result_index, selected_group)
        };
        let lazy_fetch_session = tab.table.active_lazy_fetch_session();

        self.delete_tab(tab);

        {
            let mut data = self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::maybe_shrink_tab_storage(&mut data);
        }

        if let Some(new_index) = selected_result_index {
            *self
                .active_index
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(new_index);
        } else {
            *self
                .active_index
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        }

        if let Some(group) = selected_group.as_ref() {
            if !self.data_tabs.was_deleted()
                && !group.was_deleted()
                && Self::tabs_contains_group(&self.data_tabs, group)
            {
                self.select_top_group(&self.sections.data_grid.clone());
                let _ = self.data_tabs.set_value(group);
            }
        }

        if !self.data_tabs.was_deleted() {
            self.sync_data_tab_strip_overflow_mode_after_close();
            self.data_tabs.redraw();
        }
        self.fire_on_change_callback();
        Some((index, lazy_fetch_session))
    }

    pub fn select_script_output(&mut self) {
        let _pointer_suppress_guard =
            PointerEventSuppressGuard::new(self.suppress_pointer_event_depth.clone());
        self.select_top_group(&self.sections.script_output.clone());
        Self::select_text_tab(&mut self.script_tabs, &self.script_output);
        self.fire_on_change_callback();
    }

    pub fn select_dbms_output(&mut self) {
        self.select_top_group(&self.sections.dbms_output.clone());
        self.fire_on_change_callback();
    }

    pub fn select_messages_info(&mut self) {
        self.select_top_group(&self.sections.messages.clone());
        Self::select_text_tab(&mut self.messages_tabs, &self.messages_info);
        self.fire_on_change_callback();
    }

    pub fn select_messages_errors(&mut self) {
        self.select_top_group(&self.sections.messages.clone());
        Self::select_text_tab(&mut self.messages_tabs, &self.messages_errors);
        self.fire_on_change_callback();
    }

    fn clear_support_panes(&mut self) {
        self.with_text_panes(Self::clear_pane);
    }
}

impl Default for ResultTabsWidget {
    fn default() -> Self {
        Self::new(0, 0, 100, 100)
    }
}

#[cfg(test)]
mod tests {
    use crate::db::QueryResult;
    use crate::ui::result_table::LazyFetchCallback;
    use crate::ui::sql_editor::LazyFetchRequest;
    use fltk::enums::Event;

    use super::{
        ResultTabId, ResultTabStatus, ResultTabsWidget, TableBrowseNavigation,
        TableBrowsePageRequest, TableBrowseState, TableBrowseTarget,
    };
    use crate::ui::font_settings::FONT_PROFILES;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn toad_style_tab_labels_are_fixed() {
        assert_eq!(
            ResultTabsWidget::top_level_tab_labels(),
            [
                " Data Grid ",
                " Script Output ",
                " DBMS Output ",
                " Messages "
            ]
        );
        assert_eq!(
            ResultTabsWidget::script_output_tab_labels(),
            [" Output ", " Errors "]
        );
        assert_eq!(
            ResultTabsWidget::messages_tab_labels(),
            [" Info ", " Errors "]
        );
    }

    #[test]
    fn inner_tabs_bounds_adds_top_gap() {
        assert_eq!(
            ResultTabsWidget::inner_tabs_bounds(10, 20, 100, 80),
            (10, 30, 100, 70)
        );
        assert_eq!(
            ResultTabsWidget::inner_tabs_bounds(10, 20, 100, 1),
            (10, 20, 100, 1)
        );
    }

    #[test]
    fn empty_result_tabs_consume_pointer_events() {
        assert!(ResultTabsWidget::should_consume_empty_tab_pointer_event(
            0,
            Event::Push
        ));
        assert!(ResultTabsWidget::should_consume_empty_tab_pointer_event(
            0,
            Event::Released
        ));
        assert!(ResultTabsWidget::should_consume_empty_tab_pointer_event(
            0,
            Event::MouseWheel
        ));
        assert!(!ResultTabsWidget::should_consume_empty_tab_pointer_event(
            1,
            Event::Push
        ));
        assert!(!ResultTabsWidget::should_consume_empty_tab_pointer_event(
            0,
            Event::KeyDown
        ));
    }

    #[test]
    fn result_tab_label_uses_status_and_row_count() {
        assert_eq!(
            ResultTabsWidget::result_tab_label(0, ResultTabStatus::Running, 0),
            " Running (0) "
        );
        assert_eq!(
            ResultTabsWidget::result_tab_label(1, ResultTabStatus::Fetching, 42),
            " Fetching (42) "
        );
        assert_eq!(
            ResultTabsWidget::result_tab_label(2, ResultTabStatus::Waiting, 42),
            " Waiting (42) "
        );
        assert_eq!(
            ResultTabsWidget::result_tab_label(3, ResultTabStatus::Canceling, 42),
            " Canceling (42) "
        );
        assert_eq!(
            ResultTabsWidget::result_tab_label(4, ResultTabStatus::Done, 128),
            " Done (128) "
        );
        assert_eq!(
            ResultTabsWidget::result_tab_label(5, ResultTabStatus::Error, 0),
            " Error (0) "
        );
        assert_eq!(
            ResultTabsWidget::result_tab_label(6, ResultTabStatus::Cancelled, 0),
            " Cancelled (0) "
        );
    }

    #[test]
    fn result_tab_label_uses_custom_title_when_present() {
        assert_eq!(
            ResultTabsWidget::result_tab_label_for_title(
                "Explain Plan",
                0,
                ResultTabStatus::Done,
                12
            ),
            " Explain Plan · Done (12) "
        );
        assert_eq!(
            ResultTabsWidget::result_tab_label_for_title("Result", 0, ResultTabStatus::Done, 12),
            " Done (12) "
        );
    }

    #[test]
    fn table_browse_tab_label_uses_page_number_and_current_page_row_count() {
        assert_eq!(ResultTabsWidget::table_browse_page_number(0, 500), 1);
        assert_eq!(ResultTabsWidget::table_browse_page_number(500, 500), 2);
        assert_eq!(ResultTabsWidget::table_browse_page_number(1_000, 500), 3);
        assert_eq!(
            ResultTabsWidget::table_browse_tab_label_for_title("EMP", 2, 500),
            " EMP · Page 2 (500) "
        );
        assert_eq!(
            ResultTabsWidget::table_browse_tab_label_for_title("Result", 1, 42),
            " Page 1 (42) "
        );
    }

    #[test]
    fn explain_plan_text_result_uses_text_column_only() {
        let result = ResultTabsWidget::text_result(
            "Explain Plan",
            "Plan hash value: 1\nTABLE ACCESS FULL",
            "loaded",
        );

        assert_eq!(result.columns.len(), 1);
        assert_eq!(result.columns[0].name, "Text");
        assert_eq!(
            result.rows,
            vec![
                vec!["Plan hash value: 1".to_string()],
                vec!["TABLE ACCESS FULL".to_string()],
            ]
        );
    }

    #[test]
    fn result_status_uses_shared_terminal_state_mapping() {
        let done = QueryResult::new_select("select 1", Vec::new(), Vec::new(), Duration::ZERO);
        let mut cancelled = QueryResult::new_error("select sleep", "Query cancelled");
        cancelled.message = "Query cancelled".to_string();
        let prefixed_cancelled = QueryResult::new_error("select sleep", "Query cancelled");
        let mut american_canceled = QueryResult::new_error("select sleep", "Query canceled");
        american_canceled.message = "ERROR: Query canceled".to_string();
        let error = QueryResult::new_error("select missing", "table not found");

        assert_eq!(
            ResultTabStatus::from_query_result(&done),
            ResultTabStatus::Done
        );
        assert_eq!(
            ResultTabStatus::from_query_result(&cancelled),
            ResultTabStatus::Cancelled
        );
        assert_eq!(
            ResultTabStatus::from_query_result(&prefixed_cancelled),
            ResultTabStatus::Cancelled
        );
        assert_eq!(
            ResultTabStatus::from_query_result(&american_canceled),
            ResultTabStatus::Cancelled
        );
        assert_eq!(
            ResultTabStatus::from_query_result(&error),
            ResultTabStatus::Error
        );
    }

    #[test]
    fn status_bar_message_uses_same_state_labels() {
        assert_eq!(
            ResultTabStatus::Fetching.status_bar_message_with_rows(42),
            "Fetching rows: 42"
        );
        assert_eq!(
            ResultTabStatus::Canceling.status_bar_message(),
            ResultTabStatus::Canceling.label()
        );
    }

    #[test]
    fn table_browse_request_normalization_preserves_an_explicit_page_size() {
        let target = TableBrowseTarget::new(
            crate::db::DatabaseType::MySQL,
            Some("APP".to_string()),
            "EMP".to_string(),
            "`APP`.`EMP`".to_string(),
            "APP.EMP".to_string(),
        );
        let applied_request = TableBrowsePageRequest::first(ResultTabId::new(7), target);
        let state = TableBrowseState {
            applied_request: applied_request.clone(),
            pending_request: None,
            has_next: false,
            loading: false,
            last_success: None,
            last_edit_descriptor: None,
        };

        let mut explicit = applied_request.clone();
        explicit.page_size = 100;
        state.normalize_request(&mut explicit);
        assert_eq!(explicit.page_size, 100);

        let mut inherited = applied_request;
        inherited.page_size = 0;
        state.normalize_request(&mut inherited);
        assert_eq!(inherited.page_size, 500);
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn table_browse_failure_and_close_release_loading_and_popup_resources() {
        let _app = fltk::app::App::default();
        let mut tabs = ResultTabsWidget::new(0, 0, 640, 360);
        let id = tabs.reserve_result_tab_id();
        let target = TableBrowseTarget::new(
            crate::db::DatabaseType::MySQL,
            Some("APP".to_string()),
            "EMP".to_string(),
            "`APP`.`EMP`".to_string(),
            "APP.EMP".to_string(),
        );
        assert!(tabs
            .ensure_table_browse_tab_by_id(
                id,
                target,
                Arc::new(Mutex::new(crate::ui::IntellisenseData::new())),
                500,
                true,
            )
            .is_some());

        let mut request = tabs.table_browse_initial_request(id).unwrap();
        request.navigation = TableBrowseNavigation::Last;
        tabs.begin_table_browse_request(request).unwrap();
        assert!(tabs.table_browse_is_loading(id));
        tabs.fail_table_browse_result_by_id(id);
        assert!(!tabs.table_browse_is_loading(id));

        let (_, popup) = tabs.capture_tour_show_table_browse_popup().unwrap();
        assert!(popup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_visible());
        assert!(tabs.close_tab_by_id_and_take_lazy_fetch(id).is_some());
        assert_eq!(
            popup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .popup_dimensions(),
            (0, 0)
        );
    }

    #[test]
    fn stream_updates_do_not_overwrite_terminal_or_canceling_status() {
        assert_eq!(
            ResultTabStatus::for_stream_update(ResultTabStatus::Running),
            ResultTabStatus::Fetching
        );
        assert_eq!(
            ResultTabStatus::for_stream_update(ResultTabStatus::Waiting),
            ResultTabStatus::Fetching
        );
        assert_eq!(
            ResultTabStatus::for_stream_update(ResultTabStatus::Canceling),
            ResultTabStatus::Canceling
        );
        assert_eq!(
            ResultTabStatus::for_stream_update(ResultTabStatus::Error),
            ResultTabStatus::Error
        );
        assert_eq!(
            ResultTabStatus::for_stream_update(ResultTabStatus::Cancelled),
            ResultTabStatus::Cancelled
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
                assert_eq!(session_id, 11);
                assert_eq!(request, LazyFetchRequest::Cancel);
                assert!(callback_for_assert.try_lock().is_ok());
                true
            }));

        ResultTabsWidget::invoke_lazy_fetch_callback(&callback, 11, LazyFetchRequest::Cancel);
    }

    #[test]
    #[cfg_attr(
        any(target_os = "macos", target_os = "linux"),
        ignore = "FLTK widget tests require a native UI test environment"
    )]
    fn font_changes_update_existing_and_future_result_tables() {
        let mut tabs = ResultTabsWidget::new(0, 0, 640, 360);
        tabs.append_explain_plan_tab("first");

        tabs.apply_font_settings(FONT_PROFILES[1], 23);
        tabs.append_explain_plan_tab("second");

        let sizes = tabs
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|tab| tab.table.font_size_for_test())
            .collect::<Vec<_>>();
        assert_eq!(sizes, vec![23, 23]);
    }
}
