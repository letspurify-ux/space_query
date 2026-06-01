use fltk::{
    app,
    enums::{Align, CallbackReason, CallbackTrigger, Event, FrameType, Key},
    group::{Group, Tabs, TabsOverflow},
    prelude::*,
    text::{TextBuffer, TextDisplay},
};
use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use crate::ui::constants;
use crate::ui::font_settings::{configured_editor_profile, FontProfile};
use crate::ui::result_table::{
    LazyFetchCallback, ResultGridSqlExecuteCallback, ResultTableContextActionCallback,
};
use crate::ui::text_buffer_access;
use crate::ui::theme;
use crate::ui::ResultTableWidget;

type ResultTabsChangeCallback = Box<dyn FnMut()>;
type ResultTabsCloseCallback = Box<dyn FnMut(ResultTabCloseTarget)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResultTabCloseTarget {
    Result(usize),
    ScriptOutput,
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
    explain_plan: Arc<Mutex<TextPane>>,
    font_profile: Arc<Mutex<FontProfile>>,
    font_size: Arc<Mutex<u32>>,
    max_cell_display_chars: Arc<Mutex<usize>>,
    execute_sql_callback: Arc<Mutex<Option<ResultGridSqlExecuteCallback>>>,
    lazy_fetch_callback: LazyFetchCallback,
    context_action_callback: ResultTableContextActionCallback,
    on_change_callback: Arc<Mutex<Option<ResultTabsChangeCallback>>>,
    on_close_callback: Arc<Mutex<Option<ResultTabsCloseCallback>>>,
    suppress_pointer_event_depth: Arc<Mutex<u32>>,
}

#[derive(Clone)]
struct ResultTab {
    group: Group,
    table: ResultTableWidget,
    status: ResultTabStatus,
    row_count: usize,
}

#[derive(Clone)]
struct ResultSections {
    data_grid: Group,
    script_output: Group,
    dbms_output: Group,
    messages: Group,
    explain_plan: Group,
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
        normalized.eq_ignore_ascii_case("Query cancelled")
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

    fn top_level_tab_labels() -> [&'static str; 5] {
        [
            " Data Grid ",
            " Script Output ",
            " DBMS Output ",
            " Messages ",
            " Explain Plan ",
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

    fn should_reset_tab_strip_left_anchor(child_count: i32, width: i32, height: i32) -> bool {
        child_count > 1 && width > 0 && height > 0
    }

    fn should_reapply_tab_overflow_mode_on_wheel(
        child_count: i32,
        width: i32,
        height: i32,
    ) -> bool {
        child_count > 0 && width > 0 && height > 0
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

    fn reset_tab_strip_left_anchor(&mut self) {
        // Re-applying overflow mode resets FLTK's internal tab offset,
        // keeping the visible strip anchored from the left. Skip transient
        // empty/single-tab states while tabs are being recreated because
        // overflow math is irrelevant there.
        if Self::should_reset_tab_strip_left_anchor(
            self.data_tabs.children(),
            self.data_tabs.w(),
            self.data_tabs.h(),
        ) {
            self.data_tabs.handle_overflow(TabsOverflow::Pulldown);
        } else {
            self.data_tabs.handle_overflow(TabsOverflow::Compress);
        }
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
        index: usize,
        mut group: Group,
        status: ResultTabStatus,
        row_count: usize,
    ) {
        if self.data_tabs.was_deleted() || group.was_deleted() {
            return;
        }
        group.set_label(&Self::result_tab_label(index, status, row_count));
        group.redraw();
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
                (tab.group.clone(), tab.table.clone())
            })
        };
        if let Some((group, _)) = tab_parts.as_ref() {
            self.update_tab_group_label(index, group.clone(), status, row_count);
        }
        tab_parts
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
        f(&self.explain_plan);
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
        let font_profile = Arc::new(Mutex::new(configured_editor_profile()));
        let font_size = Arc::new(Mutex::new(constants::DEFAULT_FONT_SIZE as u32));
        let max_cell_display_chars = Arc::new(Mutex::new(
            constants::RESULT_CELL_MAX_DISPLAY_CHARS_DEFAULT as usize,
        ));
        let execute_sql_callback: Arc<Mutex<Option<ResultGridSqlExecuteCallback>>> =
            Arc::new(Mutex::new(None));
        let lazy_fetch_callback: LazyFetchCallback = Arc::new(Mutex::new(None));
        let context_action_callback: ResultTableContextActionCallback = Arc::new(Mutex::new(None));
        let on_change_callback: Arc<Mutex<Option<ResultTabsChangeCallback>>> =
            Arc::new(Mutex::new(None));
        let on_close_callback: Arc<Mutex<Option<ResultTabsCloseCallback>>> =
            Arc::new(Mutex::new(None));
        let suppress_pointer_event_depth = Arc::new(Mutex::new(0u32));

        tabs.begin();
        let (section_x, section_y, section_w, section_h) = Self::content_bounds(&tabs);
        let text_profile = *font_profile
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let text_size = *font_size
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let [data_grid_label, script_output_label, dbms_output_label, messages_label, explain_label] =
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

        let explain_section =
            Self::create_section_group(section_x, section_y, section_w, section_h, explain_label);
        explain_section.begin();
        let explain_plan_pane = Self::create_text_pane(
            section_x,
            section_y,
            section_w,
            section_h,
            "",
            text_profile,
            text_size,
        );
        explain_section.resizable(&explain_plan_pane.group);
        explain_section.end();
        tabs.end();
        let _ = tabs.set_value(&data_grid_section);
        let _ = script_tabs.set_value(&script_output_pane.group);
        let _ = messages_tabs.set_value(&messages_info_pane.group);

        let sections = ResultSections {
            data_grid: data_grid_section,
            script_output: script_section,
            dbms_output: dbms_section,
            messages: messages_section,
            explain_plan: explain_section,
        };

        let script_output = Arc::new(Mutex::new(script_output_pane));
        let script_errors = Arc::new(Mutex::new(script_errors_pane));
        let dbms_output = Arc::new(Mutex::new(dbms_output_pane));
        let messages_info = Arc::new(Mutex::new(messages_info_pane));
        let messages_errors = Arc::new(Mutex::new(messages_errors_pane));
        let explain_plan = Arc::new(Mutex::new(explain_plan_pane));

        let data_for_cb = data.clone();
        let active_for_cb = active_index.clone();
        let on_change_for_cb = on_change_callback.clone();
        data_tabs.set_callback(move |t| {
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
        });

        let on_change_for_top_cb = on_change_callback.clone();
        tabs.set_callback(move |_| {
            Self::fire_on_change_with(&on_change_for_top_cb);
        });

        let suppress_pointer_for_cb = suppress_pointer_event_depth.clone();
        let tabs_for_key = tabs.clone();
        tabs.handle(move |tabs, ev| {
            if Self::should_suppress_pointer_event(&suppress_pointer_for_cb, ev) {
                return true;
            }
            if Self::should_consume_empty_tab_pointer_event(tabs.children(), ev) {
                return true;
            }
            if matches!(ev, Event::MouseWheel)
                && Self::should_reapply_tab_overflow_mode_on_wheel(
                    tabs.children(),
                    tabs.w(),
                    tabs.h(),
                )
            {
                // Prevent FLTK Tabs from applying wheel-based strip offset changes.
                // Wheel events can bubble down from nearby panes and cause the
                // result-tab header to snap right unexpectedly.
                tabs.handle_overflow(TabsOverflow::Pulldown);
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
        data_tabs.handle(move |tabs, ev| {
            if Self::should_suppress_pointer_event(&suppress_pointer_for_data_cb, ev) {
                return true;
            }
            if Self::should_consume_empty_tab_pointer_event(tabs.children(), ev) {
                return true;
            }
            if matches!(ev, Event::MouseWheel)
                && Self::should_reapply_tab_overflow_mode_on_wheel(
                    tabs.children(),
                    tabs.w(),
                    tabs.h(),
                )
            {
                tabs.handle_overflow(TabsOverflow::Pulldown);
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
        data_tabs.resize_callback(move |t, _, _, _, _| {
            Self::layout_children(t);
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
        let explain_plan_for_resize = explain_plan.clone();
        tabs.resize_callback(move |t, _, _, _, _| {
            Self::layout_children(t);
            Self::layout_inner_tabs(&sections_for_resize.data_grid, &mut data_tabs_for_resize);
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
            {
                let mut pane = explain_plan_for_resize
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                Self::layout_text_pane_in_group(&sections_for_resize.explain_plan, &mut pane);
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
            explain_plan,
            font_profile,
            font_size,
            max_cell_display_chars,
            execute_sql_callback,
            lazy_fetch_callback,
            context_action_callback,
            on_change_callback,
            on_close_callback,
            suppress_pointer_event_depth,
        }
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
        self.reset_tab_strip_left_anchor();
        self.data_tabs.redraw();
        self.fire_on_change_callback();
    }

    pub fn clear(&mut self) {
        self.clear_grids();
        self.clear_support_panes();
        self.fire_on_change_callback();
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

    pub fn lazy_fetch_session_at(&self, index: usize) -> Option<u64> {
        self.data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(index)
            .and_then(|tab| tab.table.active_lazy_fetch_session())
    }

    pub fn active_result_index(&self) -> Option<usize> {
        if !self.top_group_is_current(&self.sections.data_grid) {
            return None;
        }
        let index = *self
            .active_index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let group_matches = index
            .and_then(|idx| {
                self.data
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get(idx)
                    .map(|tab| tab.group.clone())
            })
            .is_some_and(|group| {
                self.data_tabs
                    .value()
                    .is_some_and(|current| current.as_widget_ptr() == group.as_widget_ptr())
            });
        if group_matches {
            index
        } else {
            None
        }
    }

    pub fn append_script_output_lines(&mut self, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        let should_select = self.tab_count() == 0;
        Self::append_lines_to_pane(&self.script_output, lines);
        if should_select {
            self.select_script_output();
        }
    }

    pub fn append_dbms_output_lines(&mut self, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        let should_select = self.tab_count() == 0;
        Self::append_lines_to_pane(&self.dbms_output, lines);
        if should_select {
            self.select_top_group(&self.sections.dbms_output.clone());
            self.fire_on_change_callback();
        }
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

    pub fn set_explain_plan_text(&mut self, text: &str) {
        Self::set_pane_text(&self.explain_plan, text);
        self.select_explain_plan();
    }

    pub fn start_statement(&mut self, index: usize, _label: &str) {
        let _pointer_suppress_guard =
            PointerEventSuppressGuard::new(self.suppress_pointer_event_depth.clone());
        let existing_group = self
            .set_result_tab_state(index, ResultTabStatus::Running, 0)
            .map(|(group, _)| group);
        if let Some(group) = existing_group {
            // Extract the group before calling set_value to avoid re-entrant borrow
            // when the tabs callback fires
            self.select_top_group(&self.sections.data_grid.clone());
            let _ = self.data_tabs.set_value(&group);
            *self
                .active_index
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(index);
            return;
        }

        self.data_tabs.begin();
        // Use explicit tab content bounds to avoid relying on hard-coded header height.
        let (x, y, w, h) = Self::content_bounds(&self.data_tabs);
        let mut group = Group::new(x, y, w, h, None).with_label(&Self::result_tab_label(
            index,
            ResultTabStatus::Running,
            0,
        ));
        group.set_color(theme::panel_bg());
        group.set_selection_color(theme::panel_bg());
        group.set_label_color(theme::text_secondary());
        group.set_align(Align::Center | Align::Inside);
        group.set_trigger(CallbackTrigger::Closed);
        let group_ptr = group.as_widget_ptr() as usize;
        let data_for_close = self.data.clone();
        let on_close_for_group = self.on_close_callback.clone();
        group.set_callback(move |_| {
            if app::callback_reason() != CallbackReason::Closed {
                return;
            }
            let index = data_for_close
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .position(|tab| tab.group.as_widget_ptr() as usize == group_ptr);
            if let Some(index) = index {
                Self::fire_on_close_with(&on_close_for_group, ResultTabCloseTarget::Result(index));
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
                group,
                table,
                status: ResultTabStatus::Running,
                row_count: 0,
            });
            let idx = data.len().saturating_sub(1);
            let group = data.get(idx).map(|tab| tab.group.clone());
            (idx, group)
        };
        // Extract the group before calling set_value to avoid re-entrant borrow
        // when the tabs callback fires
        if let Some(group) = new_group {
            self.select_top_group(&self.sections.data_grid.clone());
            let _ = self.data_tabs.set_value(&group);
        }
        self.reset_tab_strip_left_anchor();
        *self
            .active_index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(new_index);
        self.fire_on_change_callback();
    }

    pub fn start_streaming(&mut self, index: usize, columns: &[String], null_text: &str) {
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
        }
        self.fire_on_change_callback();
    }

    pub fn append_rows(&mut self, index: usize, rows: Vec<Vec<String>>) {
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

    pub fn finish_streaming(&mut self, index: usize) {
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

    pub fn set_lazy_fetch_session(&mut self, index: usize, session_id: u64) {
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

    pub fn mark_lazy_fetch_waiting(&mut self, index: usize) {
        let row_count = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(index)
            .map(|tab| tab.row_count);
        if let Some(row_count) = row_count {
            self.set_result_tab_state(index, ResultTabStatus::Waiting, row_count);
        }
        self.fire_on_change_callback();
    }

    pub fn mark_statement_canceling(&mut self, index: usize) {
        self.mark_statement_status(index, ResultTabStatus::Canceling);
    }

    pub fn mark_statement_cancelled(&mut self, index: usize) {
        self.mark_statement_status(index, ResultTabStatus::Cancelled);
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

    pub fn clear_lazy_fetch_session(&mut self, index: usize, session_id: u64, run_pending: bool) {
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

    pub fn align_tab_strip_left(&mut self) {
        self.reset_tab_strip_left_anchor();
        self.tabs.redraw();
    }

    pub fn display_result(&mut self, index: usize, result: &crate::db::QueryResult) {
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

    pub fn finish_result_status(&mut self, index: usize, result: &crate::db::QueryResult) {
        let status = ResultTabStatus::from_query_result(result);
        let _ = self.set_result_tab_state(index, status, result.row_count);
        self.fire_on_change_callback();
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

    pub fn export_to_csv_after_fetch_all(
        &self,
        callback: Box<dyn FnMut(String, usize)>,
    ) -> Option<(String, usize)> {
        self.current_table()
            .and_then(|table| table.export_to_csv_after_fetch_all(callback))
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

        // Step 1: Cleanup the table widget (clears callbacks and data buffers)
        tab.table.cleanup();

        // Step 2 & 3: Explicitly remove/delete the table widget first to ensure
        // callback closures are dropped immediately, then clear/delete any
        // additional child widgets that may be added to result tabs in the future.
        let mut group = tab.group;
        let table_widget = tab.table.get_widget();
        if !group.was_deleted() && !table_widget.was_deleted() && group.find(&table_widget) >= 0 {
            group.remove(&table_widget);
        }
        if !table_widget.was_deleted() {
            fltk::table::Table::delete(table_widget);
        }
        if !group.was_deleted() {
            group.clear();
        }

        // Step 4: Remove group from tabs and delete
        if !self.data_tabs.was_deleted() && !group.was_deleted() && self.data_tabs.find(&group) >= 0
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

    pub fn close_current_tab_and_take_lazy_fetch(&mut self) -> Option<(usize, Option<u64>)> {
        let index = (*self
            .active_index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))?;

        self.close_tab_at_and_take_lazy_fetch(index)
    }

    pub fn close_tab_at_and_take_lazy_fetch(
        &mut self,
        index: usize,
    ) -> Option<(usize, Option<u64>)> {
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

        if let Some(group) = selected_group.as_ref() {
            if !self.data_tabs.was_deleted() && !group.was_deleted() {
                self.select_top_group(&self.sections.data_grid.clone());
                let _ = self.data_tabs.set_value(group);
            }
        }

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

        if !self.data_tabs.was_deleted() {
            self.reset_tab_strip_left_anchor();
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

    pub fn select_data_grid(&mut self, index: usize) {
        let group = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(index)
            .map(|tab| tab.group.clone());
        let Some(group) = group else {
            return;
        };
        self.select_top_group(&self.sections.data_grid.clone());
        if !self.data_tabs.was_deleted() && !group.was_deleted() {
            let _ = self.data_tabs.set_value(&group);
        }
        *self
            .active_index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(index);
        self.fire_on_change_callback();
    }

    pub fn select_messages_errors(&mut self) {
        self.select_top_group(&self.sections.messages.clone());
        Self::select_text_tab(&mut self.messages_tabs, &self.messages_errors);
        self.fire_on_change_callback();
    }

    pub fn select_explain_plan(&mut self) {
        self.select_top_group(&self.sections.explain_plan.clone());
        self.fire_on_change_callback();
    }

    pub fn clear_current_support_section(&mut self) -> bool {
        if self.top_group_is_current(&self.sections.script_output) {
            Self::clear_pane(&self.script_output);
            Self::clear_pane(&self.script_errors);
        } else if self.top_group_is_current(&self.sections.dbms_output) {
            Self::clear_pane(&self.dbms_output);
        } else if self.top_group_is_current(&self.sections.messages) {
            Self::clear_pane(&self.messages_info);
            Self::clear_pane(&self.messages_errors);
        } else if self.top_group_is_current(&self.sections.explain_plan) {
            Self::clear_pane(&self.explain_plan);
        } else {
            return false;
        }
        self.fire_on_change_callback();
        true
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

    use super::{ResultTabStatus, ResultTabsWidget};
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
                " Messages ",
                " Explain Plan "
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
    fn tab_strip_left_anchor_reset_requires_multi_tab_layout() {
        assert!(!ResultTabsWidget::should_reset_tab_strip_left_anchor(
            0, 320, 240
        ));
        assert!(!ResultTabsWidget::should_reset_tab_strip_left_anchor(
            1, 320, 240
        ));
        assert!(ResultTabsWidget::should_reset_tab_strip_left_anchor(
            2, 320, 240
        ));
    }

    #[test]
    fn mouse_wheel_overflow_reapply_allows_single_tab() {
        assert!(!ResultTabsWidget::should_reapply_tab_overflow_mode_on_wheel(0, 320, 240));
        assert!(ResultTabsWidget::should_reapply_tab_overflow_mode_on_wheel(
            1, 320, 240
        ));
        assert!(!ResultTabsWidget::should_reapply_tab_overflow_mode_on_wheel(1, 0, 240));
        assert!(!ResultTabsWidget::should_reapply_tab_overflow_mode_on_wheel(1, 320, 0));
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
}
