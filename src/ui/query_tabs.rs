use fltk::{
    app,
    enums::{Align, CallbackReason, CallbackTrigger, Event, FrameType},
    group::{Group, Tabs, TabsOverflow},
    prelude::*,
};
use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use crate::ui::constants::TAB_HEADER_HEIGHT;
use crate::ui::tab_strip;
use crate::ui::theme;

pub type QueryTabId = u64;
type TabSelectCallback = Box<dyn FnMut(QueryTabId)>;
type TabCloseCallback = Box<dyn FnMut(QueryTabId)>;

#[derive(Clone)]
pub struct QueryTabsWidget {
    tabs: Tabs,
    entries: Arc<Mutex<Vec<TabEntry>>>,
    next_id: Arc<Mutex<QueryTabId>>,
    on_select: Arc<Mutex<Option<TabSelectCallback>>>,
    on_close: Arc<Mutex<Option<TabCloseCallback>>>,
    suppress_select_callback_depth: Arc<Mutex<u32>>,
    suppress_pointer_event_depth: Arc<Mutex<u32>>,
    tab_strip_state: Arc<Mutex<tab_strip::TabStripState>>,
}

#[derive(Clone)]
struct TabEntry {
    id: QueryTabId,
    group: Group,
}

struct CallbackSuppressGuard {
    counter: Arc<Mutex<u32>>,
}

impl CallbackSuppressGuard {
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

impl Drop for CallbackSuppressGuard {
    fn drop(&mut self) {
        let mut guard = self
            .counter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = guard.saturating_sub(1);
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

impl QueryTabsWidget {
    fn panic_payload_to_string(payload: &(dyn Any + Send)) -> String {
        if let Some(message) = payload.downcast_ref::<&str>() {
            (*message).to_string()
        } else if let Some(message) = payload.downcast_ref::<String>() {
            message.clone()
        } else {
            "unknown panic payload".to_string()
        }
    }

    fn invoke_on_select_callback(
        callback_slot: &Arc<Mutex<Option<TabSelectCallback>>>,
        tab_id: QueryTabId,
    ) {
        let callback = {
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.take()
        };

        if let Some(mut cb) = callback {
            let callback_result = panic::catch_unwind(AssertUnwindSafe(|| cb(tab_id)));
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.is_none() {
                *slot = Some(cb);
            }
            if let Err(payload) = callback_result {
                let panic_payload = Self::panic_payload_to_string(payload.as_ref());
                crate::utils::logging::log_error(
                    "query_tabs::callback",
                    &format!("tab select callback panicked: {panic_payload}"),
                );
                eprintln!("tab select callback panicked: {panic_payload}");
            }
        }
    }

    fn invoke_on_close_callback(
        callback_slot: &Arc<Mutex<Option<TabCloseCallback>>>,
        tab_id: QueryTabId,
    ) {
        let callback = {
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot.take()
        };

        if let Some(mut cb) = callback {
            let callback_result = panic::catch_unwind(AssertUnwindSafe(|| cb(tab_id)));
            let mut slot = callback_slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.is_none() {
                *slot = Some(cb);
            }
            if let Err(payload) = callback_result {
                let panic_payload = Self::panic_payload_to_string(payload.as_ref());
                crate::utils::logging::log_error(
                    "query_tabs::callback",
                    &format!("tab close callback panicked: {panic_payload}"),
                );
                eprintln!("tab close callback panicked: {panic_payload}");
            }
        }
    }

    fn content_bounds(tabs: &Tabs) -> (i32, i32, i32, i32) {
        // Keep a stable tab-header height regardless of surrounding splitter drags.
        // This avoids top/bottom header bar height jitter while panes are resized.
        let x = tabs.x();
        let y = tabs.y() + TAB_HEADER_HEIGHT;
        let w = tabs.w();
        let h = tabs.h() - TAB_HEADER_HEIGHT;
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

    fn sync_tab_strip_overflow_mode_for(
        tabs: &mut Tabs,
        tab_strip_state: &Arc<Mutex<tab_strip::TabStripState>>,
    ) {
        let _ = tab_strip::try_with_state(tab_strip_state, |state| {
            tab_strip::sync_overflow_mode(tabs, state, TAB_HEADER_HEIGHT);
        });
    }

    fn record_tab_strip_selection_for(
        tabs: &mut Tabs,
        tab_strip_state: &Arc<Mutex<tab_strip::TabStripState>>,
    ) {
        let _ = tab_strip::try_with_state(tab_strip_state, |state| {
            tab_strip::record_selected_tab(tabs, state, TAB_HEADER_HEIGHT);
        });
    }

    fn record_tab_strip_removal_for(
        group: &Group,
        tab_strip_state: &Arc<Mutex<tab_strip::TabStripState>>,
    ) {
        let _ = tab_strip::try_with_state(tab_strip_state, |state| {
            tab_strip::record_removed_tab(state, group.as_widget_ptr() as usize);
        });
    }

    fn sync_tab_strip_overflow_mode(&mut self) {
        Self::sync_tab_strip_overflow_mode_for(&mut self.tabs, &self.tab_strip_state);
    }

    fn sync_tab_strip_overflow_mode_after_close(&self) {
        let mut tabs = self.tabs.clone();
        let tab_strip_state = self.tab_strip_state.clone();
        crate::ui::ui_timeout::schedule(0.0, move || {
            if tabs.was_deleted() {
                return;
            }
            Self::sync_tab_strip_overflow_mode_for(&mut tabs, &tab_strip_state);
            tabs.redraw();
        });
    }

    fn maybe_shrink_entry_storage(entries: &mut Vec<TabEntry>) {
        // Closing many tabs can leave tab metadata capacity heavily over-allocated.
        // Shrink only when substantially over-provisioned to avoid churn.
        let len = entries.len();
        let capacity = entries.capacity();
        if len == 0 || (capacity > 0 && len.saturating_mul(2) < capacity) {
            entries.shrink_to_fit();
        }
    }

    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        let mut tabs = Tabs::new(x, y, w, h, None);
        tabs.end();
        tabs.set_color(theme::panel_bg());
        tabs.set_selection_color(theme::selection_soft());
        tabs.set_frame(FrameType::RFlatBox);
        tabs.set_label_color(theme::text_secondary());
        tabs.set_label_size((TAB_HEADER_HEIGHT - 8).max(8));
        // Center labels in tab headers.
        tabs.set_tab_align(Align::Center);
        // Start without a movable tab offset. The shared strip logic enables
        // Pulldown only when the selected tab has a stable viewport.
        tabs.handle_overflow(TabsOverflow::Compress);

        let entries = Arc::new(Mutex::new(Vec::<TabEntry>::new()));
        let next_id = Arc::new(Mutex::new(1u64));
        let on_select = Arc::new(Mutex::new(None::<TabSelectCallback>));
        let on_close = Arc::new(Mutex::new(None::<TabCloseCallback>));
        let suppress_select_callback_depth = Arc::new(Mutex::new(0u32));
        let suppress_pointer_event_depth = Arc::new(Mutex::new(0u32));
        let tab_strip_state = Arc::new(Mutex::new(tab_strip::TabStripState::default()));

        let entries_for_cb = entries.clone();
        let on_select_for_cb = on_select.clone();
        let suppress_for_cb = suppress_select_callback_depth.clone();
        let suppress_pointer_for_cb = suppress_pointer_event_depth.clone();
        let tab_strip_state_for_cb = tab_strip_state.clone();
        tabs.set_callback(move |tabs| {
            Self::record_tab_strip_selection_for(tabs, &tab_strip_state_for_cb);
            if *suppress_for_cb
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                > 0
            {
                return;
            }
            let Some(selected) = tabs.value() else {
                return;
            };
            let selected_ptr = selected.as_widget_ptr();
            let selected_id = entries_for_cb
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .find(|entry| entry.group.as_widget_ptr() == selected_ptr)
                .map(|entry| entry.id);
            if let Some(tab_id) = selected_id {
                Self::invoke_on_select_callback(&on_select_for_cb, tab_id);
            }
        });
        let tab_strip_state_for_resize = tab_strip_state.clone();
        tabs.resize_callback(move |t, _, _, _, _| {
            Self::layout_children(t);
            Self::sync_tab_strip_overflow_mode_for(t, &tab_strip_state_for_resize);
        });
        // Run this filter before FLTK's built-in Tabs handler. Header wheel
        // events must be consumed before FLTK mutates its internal tab offset,
        // while body wheel events must still reach the active editor.
        tabs.super_handle_first(false);
        let tab_strip_state_for_handle = tab_strip_state.clone();
        let tab_strip_pointer_gesture =
            Arc::new(Mutex::new(tab_strip::TabStripPointerGesture::default()));
        tabs.handle(move |tabs, ev| {
            // Once a header press has switched FLTK to native-first handling,
            // cleanup/reset must run even if a nested programmatic operation is
            // temporarily suppressing pointer events.
            if matches!(
                ev,
                Event::Drag
                    | Event::Released
                    | Event::Unfocus
                    | Event::Deactivate
                    | Event::Hide
                    | Event::MouseWheel
            ) {
                let handled = tab_strip::try_with_state(&tab_strip_state_for_handle, |state| {
                    let mut gesture = tab_strip_pointer_gesture
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    tab_strip::handle_pointer_event(
                        tabs,
                        ev,
                        state,
                        &mut gesture,
                        TAB_HEADER_HEIGHT,
                    )
                });
                if handled == Some(true)
                    || (handled.is_none()
                        && ev == Event::MouseWheel
                        && tab_strip::should_consume_mouse_wheel_for_tabs(tabs, TAB_HEADER_HEIGHT))
                {
                    return true;
                }
            }
            if Self::should_suppress_pointer_event(&suppress_pointer_for_cb, ev) {
                return true;
            }
            if ev == Event::Push
                && tab_strip::try_with_state(&tab_strip_state_for_handle, |state| {
                    let mut gesture = tab_strip_pointer_gesture
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    tab_strip::handle_pointer_event(
                        tabs,
                        ev,
                        state,
                        &mut gesture,
                        TAB_HEADER_HEIGHT,
                    )
                }) == Some(true)
            {
                return true;
            }
            false
        });

        Self {
            tabs,
            entries,
            next_id,
            on_select,
            on_close,
            suppress_select_callback_depth,
            suppress_pointer_event_depth,
            tab_strip_state,
        }
    }

    pub fn set_on_select<F>(&mut self, callback: F)
    where
        F: FnMut(QueryTabId) + 'static,
    {
        *self
            .on_select
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn set_on_close<F>(&mut self, callback: F)
    where
        F: FnMut(QueryTabId) + 'static,
    {
        *self
            .on_close
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(callback));
    }

    pub fn get_widget(&self) -> Tabs {
        self.tabs.clone()
    }

    pub(crate) fn refresh_tab_strip_overflow_mode(&mut self) {
        if self.tabs.was_deleted() {
            return;
        }
        self.sync_tab_strip_overflow_mode();
        self.tabs.redraw();
    }

    pub fn add_tab(&mut self, label: &str) -> QueryTabId {
        let tab_id = {
            let mut next = self
                .next_id
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let id = *next;
            *next = next.saturating_add(1);
            id
        };
        self.tabs.begin();
        let (x, y, w, h) = Self::content_bounds(&self.tabs);
        let mut group = Group::new(x, y, w, h, None).with_label(&Self::display_label(label));
        group.set_color(theme::panel_bg());
        group.set_selection_color(theme::panel_bg());
        group.set_label_color(theme::text_secondary());
        group.set_align(Align::Center | Align::Inside);
        group.set_trigger(CallbackTrigger::Closed);
        let on_close_for_group = self.on_close.clone();
        group.set_callback(move |_| {
            if app::callback_reason() == CallbackReason::Closed {
                Self::invoke_on_close_callback(&on_close_for_group, tab_id);
            }
        });
        group.end();
        self.tabs.end();

        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(TabEntry {
                id: tab_id,
                group: group.clone(),
            });
        let _pointer_suppress_guard =
            PointerEventSuppressGuard::new(self.suppress_pointer_event_depth.clone());
        let _suppress_guard =
            CallbackSuppressGuard::new(self.suppress_select_callback_depth.clone());
        let _ = self.tabs.set_value(&group);
        self.sync_tab_strip_overflow_mode();
        Self::layout_children(&self.tabs);
        self.tabs.redraw();
        tab_id
    }

    pub fn select(&mut self, tab_id: QueryTabId) {
        if let Some(group) = self.tab_group(tab_id) {
            let _pointer_suppress_guard =
                PointerEventSuppressGuard::new(self.suppress_pointer_event_depth.clone());
            let _suppress_guard =
                CallbackSuppressGuard::new(self.suppress_select_callback_depth.clone());
            let _ = self.tabs.set_value(&group);
            Self::record_tab_strip_selection_for(&mut self.tabs, &self.tab_strip_state);
            self.tabs.redraw();
        }
    }

    pub fn selected_id(&self) -> Option<QueryTabId> {
        let selected = self.tabs.value()?;
        let selected_ptr = selected.as_widget_ptr();
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|entry| entry.group.as_widget_ptr() == selected_ptr)
            .map(|entry| entry.id)
    }

    pub fn set_tab_label(&mut self, tab_id: QueryTabId, label: &str) {
        if let Some(group) = self.tab_group(tab_id) {
            let _pointer_suppress_guard =
                PointerEventSuppressGuard::new(self.suppress_pointer_event_depth.clone());
            let mut group = group;
            group.set_label(&Self::display_label(label));
            group.set_align(Align::Center | Align::Inside);
            self.sync_tab_strip_overflow_mode();
            self.tabs.redraw();
        }
    }

    pub fn close_tab(&mut self, tab_id: QueryTabId) -> bool {
        let (group, replacement_group) = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(index) = entries.iter().position(|entry| entry.id == tab_id) else {
                return false;
            };
            let group = entries.remove(index).group;
            // Fl_Tabs keeps only its selected child locally visible. Reading
            // that flag avoids calling Fl_Tabs::value() while its close
            // callback is still unwinding.
            let replacement = group
                .visible()
                .then(|| {
                    entries
                        .get(index)
                        .or_else(|| index.checked_sub(1).and_then(|prev| entries.get(prev)))
                        .map(|entry| entry.group.clone())
                })
                .flatten();
            Self::maybe_shrink_entry_storage(&mut entries);
            (group, replacement)
        };

        let _pointer_suppress_guard =
            PointerEventSuppressGuard::new(self.suppress_pointer_event_depth.clone());
        let _suppress_guard =
            CallbackSuppressGuard::new(self.suppress_select_callback_depth.clone());
        Self::record_tab_strip_removal_for(&group, &self.tab_strip_state);
        if !self.tabs.was_deleted() && self.tabs.find(&group) < self.tabs.children() {
            self.tabs.remove(&group);
        }
        if !group.was_deleted() {
            fltk::group::Group::delete(group);
        }
        if let Some(replacement_group) = replacement_group {
            if !replacement_group.was_deleted()
                && self.tabs.find(&replacement_group) < self.tabs.children()
            {
                let _ = self.tabs.set_value(&replacement_group);
            }
        }
        self.sync_tab_strip_overflow_mode_after_close();
        Self::layout_children(&self.tabs);
        self.tabs.redraw();
        true
    }

    pub fn tab_group(&self, tab_id: QueryTabId) -> Option<Group> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|entry| entry.id == tab_id)
            .map(|entry| entry.group.clone())
    }

    pub fn tab_ids(&self) -> Vec<QueryTabId> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|entry| entry.id)
            .collect()
    }

    fn display_label(label: &str) -> String {
        format!(" {label} ")
    }
}

impl Default for QueryTabsWidget {
    fn default() -> Self {
        Self::new(0, 0, 100, 100)
    }
}

#[cfg(test)]
mod tests {
    use super::QueryTabsWidget;
    use fltk::enums::Event;
    use std::sync::{Arc, Mutex};

    #[test]
    fn pointer_event_suppression_only_applies_to_mouse_driven_tab_events() {
        let depth = Arc::new(Mutex::new(1u32));

        assert!(QueryTabsWidget::should_suppress_pointer_event(
            &depth,
            Event::Move
        ));
        assert!(QueryTabsWidget::should_suppress_pointer_event(
            &depth,
            Event::Push
        ));
        assert!(QueryTabsWidget::should_suppress_pointer_event(
            &depth,
            Event::Released
        ));
        assert!(!QueryTabsWidget::should_suppress_pointer_event(
            &depth,
            Event::KeyDown
        ));
    }

    #[test]
    fn display_label_adds_single_space_padding() {
        assert_eq!(QueryTabsWidget::display_label("Query 1"), " Query 1 ");
        assert_eq!(
            QueryTabsWidget::display_label("worksheet.sql*"),
            " worksheet.sql* "
        );
    }
}
