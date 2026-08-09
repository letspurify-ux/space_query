use std::collections::HashSet;
use std::sync::{Mutex, TryLockError};

use fltk::{
    app, draw,
    enums::{CallbackTrigger, Color, Event, LabelType},
    group::{Tabs, TabsOverflow},
    prelude::*,
};

use crate::ui::theme;
use crate::utils::arithmetic::safe_div;

// These values mirror Fl_Tabs.cxx in FLTK 1.5.x.
const FLTK_TABS_BORDER: i32 = 2;
const FLTK_TABS_OVERFLOW_BUTTON_BORDER: i32 = 2;
const FLTK_TABS_EXTRA_SPACE: i32 = 10;
const FLTK_TABS_CLOSE_EXTRA_GAP: i32 = 2;
const FLTK_TABS_SELECTED_MARGIN: i32 = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
struct TabGeometry {
    widget_ptr: usize,
    left: i32,
    width: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TabStripGeometry {
    header_height: i32,
    tabs_width: i32,
    available_width: i32,
    total_width: i32,
    tabs: Vec<TabGeometry>,
}

impl TabStripGeometry {
    fn is_overflowing(&self) -> bool {
        should_use_pulldown(
            self.tabs.len() as i32,
            self.total_width,
            self.available_width,
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TabOffsetAnchor {
    #[default]
    Zero,
    Left(usize),
    Right(usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AnchoredTabOffset {
    anchor: TabOffsetAnchor,
    offset: i32,
}

#[derive(Debug, Default)]
pub(crate) struct TabStripState {
    pulldown_active: bool,
    geometry: Option<TabStripGeometry>,
    anchored_offset: AnchoredTabOffset,
    removed_widget_ptrs: HashSet<usize>,
}

#[derive(Debug, Default)]
pub(crate) struct TabStripPointerGesture {
    active: bool,
    dragged: bool,
}

pub(crate) fn try_with_state<R>(
    shared: &Mutex<TabStripState>,
    f: impl FnOnce(&mut TabStripState) -> R,
) -> Option<R> {
    // FLTK can synchronously dispatch focus/visibility events from set_value().
    // Skip that nested access instead of blocking the UI thread on its own mutex.
    let mut state = match shared.try_lock() {
        Ok(state) => state,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => return None,
    };
    Some(f(&mut state))
}

fn tab_header_height(tabs: &Tabs, fallback: i32) -> i32 {
    let fallback = fallback.clamp(0, tabs.h().max(0));
    tabs.clone()
        .into_iter()
        .filter(|child| !child.was_deleted())
        .map(|child| child.y().saturating_sub(tabs.y()))
        .filter(|height| *height >= 0)
        .min()
        .unwrap_or(fallback)
        .clamp(0, tabs.h().max(0))
}

fn should_consume_mouse_wheel(
    child_count: i32,
    tabs_x: i32,
    tabs_y: i32,
    tabs_width: i32,
    tabs_height: i32,
    header_height: i32,
    event_x: i32,
    event_y: i32,
) -> bool {
    if child_count <= 0 || tabs_width <= 0 || tabs_height <= 0 {
        return true;
    }

    let right = tabs_x.saturating_add(tabs_width);
    let bottom = tabs_y.saturating_add(tabs_height);
    let header_bottom = tabs_y.saturating_add(header_height.clamp(0, tabs_height.max(0)));

    // FLTK treats event_y == header_bottom as part of the tab bar. Only let
    // its default handler run for a point strictly inside the content body.
    // Consuming points outside the Tabs bounds is intentional: Fl_Group sends
    // unhandled wheel events to children that are not under the pointer.
    let inside_content =
        event_x >= tabs_x && event_x < right && event_y > header_bottom && event_y < bottom;
    !inside_content
}

pub(crate) fn should_consume_mouse_wheel_for_tabs(
    tabs: &Tabs,
    fallback_header_height: i32,
) -> bool {
    should_consume_mouse_wheel(
        tabs.children(),
        tabs.x(),
        tabs.y(),
        tabs.w(),
        tabs.h(),
        tab_header_height(tabs, fallback_header_height),
        app::event_x(),
        app::event_y(),
    )
}

fn point_is_in_tab_header(
    tabs_x: i32,
    tabs_y: i32,
    tabs_width: i32,
    tabs_height: i32,
    header_height: i32,
    event_x: i32,
    event_y: i32,
) -> bool {
    if tabs_width <= 0 || tabs_height <= 0 || header_height <= 0 {
        return false;
    }

    let right = tabs_x.saturating_add(tabs_width);
    let header_bottom = tabs_y.saturating_add(header_height.clamp(0, tabs_height.max(0)));
    event_x >= tabs_x && event_x < right && event_y >= tabs_y && event_y <= header_bottom
}

fn point_is_in_pulldown_button(
    tabs_x: i32,
    tabs_width: i32,
    header_height: i32,
    event_x: i32,
) -> bool {
    let button_left = tabs_x
        .saturating_add(tabs_width)
        .saturating_sub(header_height)
        .saturating_add(FLTK_TABS_OVERFLOW_BUTTON_BORDER);
    event_x >= button_left && event_x < tabs_x.saturating_add(tabs_width)
}

fn tab_width(label_width: i32, tabs_label_size: i32, closeable: bool) -> i32 {
    let close_width = if closeable {
        safe_div(tabs_label_size.max(0), 2) + FLTK_TABS_CLOSE_EXTRA_GAP
    } else {
        0
    };
    label_width
        .max(0)
        .saturating_add(close_width)
        .saturating_add(FLTK_TABS_EXTRA_SPACE)
}

fn shortcut_label_text(label: &str) -> String {
    let mut displayed = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '&' {
            displayed.push(ch);
            continue;
        }
        match chars.peek() {
            Some('&') => {
                chars.next();
                displayed.push('&');
            }
            Some(_) => {}
            None => displayed.push('&'),
        }
    }
    displayed
}

fn tab_label_width(child: &fltk::widget::Widget) -> i32 {
    let label = child.label();
    if !label.contains('&') || child.label_type() != LabelType::Normal || child.image().is_some() {
        return child.measure_label().0.max(0);
    }

    // Fl_Tabs measures labels with shortcut processing enabled, where a
    // single '&' marks (and does not occupy space before) the next glyph.
    // Widget::measure_label() normally measures the literal '&', so reproduce
    // the displayed text for an exact overflow threshold.
    let previous_font = draw::font();
    let previous_size = draw::size();
    draw::set_font(child.label_font(), child.label_size());
    let (width, _) = draw::measure(&shortcut_label_text(&label), true);
    draw::set_font(previous_font, previous_size);
    width.max(0)
}

fn has_overflow(total_width: i32, available_width: i32) -> bool {
    total_width > available_width.max(0)
}

fn should_use_pulldown(child_count: i32, total_width: i32, available_width: i32) -> bool {
    child_count > 1 && has_overflow(total_width, available_width)
}

fn natural_available_width(tabs_width: i32, frame_width: i32) -> i32 {
    // Match Fl_Tabs::tab_positions() in Compress mode. The Pulldown button
    // consumes header width only after natural tabs have exceeded this limit;
    // reserving it here would switch modes one button-width too early.
    tabs_width.saturating_sub(frame_width).max(0)
}

fn overflow_sync_mode(
    was_pulldown: bool,
    use_pulldown: bool,
    geometry_changed: bool,
) -> Option<TabsOverflow> {
    if was_pulldown != use_pulldown {
        if use_pulldown {
            Some(TabsOverflow::Pulldown)
        } else {
            Some(TabsOverflow::Compress)
        }
    } else if use_pulldown && geometry_changed {
        Some(TabsOverflow::Pulldown)
    } else {
        None
    }
}

fn tab_strip_geometry(tabs: &Tabs, fallback_header_height: i32) -> Option<TabStripGeometry> {
    if tabs.was_deleted() || tabs.w() <= 0 || tabs.h() <= 0 {
        return None;
    }

    let header_height = tab_header_height(tabs, fallback_header_height);
    let leading_width = tabs.frame().dx();
    let mut total_width = leading_width;
    let mut tab_geometries = Vec::with_capacity(tabs.children().max(0) as usize);
    for child in tabs.clone().into_iter() {
        if child.was_deleted() {
            continue;
        }
        let width = tab_width(
            tab_label_width(&child),
            tabs.label_size(),
            child.trigger().contains(CallbackTrigger::Closed),
        )
        .saturating_add(FLTK_TABS_BORDER);
        let left = total_width;
        total_width = total_width.saturating_add(width);
        tab_geometries.push(TabGeometry {
            widget_ptr: child.as_widget_ptr() as usize,
            left,
            width,
        });
    }

    Some(TabStripGeometry {
        header_height,
        tabs_width: tabs.w(),
        available_width: natural_available_width(tabs.w(), tabs.frame().dw()),
        total_width,
        tabs: tab_geometries,
    })
}

fn tab_index(geometry: &TabStripGeometry, widget_ptr: usize) -> Option<usize> {
    geometry
        .tabs
        .iter()
        .position(|tab| tab.widget_ptr == widget_ptr)
}

fn selection_margin(geometry: &TabStripGeometry, index: usize) -> i32 {
    if index == 0 || index + 1 == geometry.tabs.len() {
        FLTK_TABS_BORDER
    } else {
        FLTK_TABS_SELECTED_MARGIN
    }
}

fn pulldown_right_margin(geometry: &TabStripGeometry, selection_margin: i32) -> i32 {
    selection_margin.saturating_add(
        geometry
            .header_height
            .saturating_sub(FLTK_TABS_OVERFLOW_BUTTON_BORDER)
            .saturating_abs(),
    )
}

fn tab_can_stably_fit(geometry: &TabStripGeometry, index: usize) -> bool {
    let actual_width = geometry.tabs[index]
        .width
        .saturating_sub(FLTK_TABS_BORDER)
        .max(0);
    let margin = selection_margin(geometry, index);
    let right_margin = pulldown_right_margin(geometry, margin);

    // Fl_Tabs::value() has a fixed offset only when the selected tab plus
    // both visibility margins fit. Otherwise repeated redraws alternate
    // between its left- and right-edge corrections.
    actual_width
        .saturating_add(margin)
        .saturating_add(right_margin)
        <= geometry.tabs_width
}

fn selected_tab_can_stably_fit(geometry: &TabStripGeometry, selected_ptr: usize) -> bool {
    tab_index(geometry, selected_ptr).is_some_and(|index| tab_can_stably_fit(geometry, index))
}

fn should_use_stable_pulldown(geometry: &TabStripGeometry, selected_ptr: Option<usize>) -> bool {
    geometry.is_overflowing()
        && selected_ptr
            .is_some_and(|selected_ptr| selected_tab_can_stably_fit(geometry, selected_ptr))
}

fn apply_selected_tab_offset_at_index(
    geometry: &TabStripGeometry,
    current: AnchoredTabOffset,
    index: usize,
) -> AnchoredTabOffset {
    let selected_ptr = geometry.tabs[index].widget_ptr;
    let left = geometry.tabs[index].left;
    // TabGeometry::width includes FLTK's inter-tab BORDER advance, while
    // Fl_Tabs::value() tests the actual tab width without that final advance.
    let width = geometry.tabs[index]
        .width
        .saturating_sub(FLTK_TABS_BORDER)
        .max(0);
    let margin = selection_margin(geometry, index);
    let right_margin = pulldown_right_margin(geometry, margin);

    if left
        .saturating_add(width)
        .saturating_add(current.offset)
        .saturating_add(right_margin)
        > geometry.tabs_width
    {
        AnchoredTabOffset {
            anchor: TabOffsetAnchor::Right(selected_ptr),
            offset: geometry
                .tabs_width
                .saturating_sub(left)
                .saturating_sub(width)
                .saturating_sub(right_margin),
        }
    } else if left.saturating_add(current.offset).saturating_sub(margin) < 0 {
        AnchoredTabOffset {
            anchor: TabOffsetAnchor::Left(selected_ptr),
            offset: margin.saturating_sub(left),
        }
    } else {
        current
    }
}

fn apply_selected_tab_offset(
    geometry: &TabStripGeometry,
    current: AnchoredTabOffset,
    selected_ptr: usize,
) -> AnchoredTabOffset {
    tab_index(geometry, selected_ptr)
        .map(|index| apply_selected_tab_offset_at_index(geometry, current, index))
        .unwrap_or(current)
}

#[derive(Clone, Copy)]
enum IndexedTabOffsetAnchor {
    Zero,
    Left(usize),
    Right(usize),
}

fn indexed_tab_offset_anchor(
    geometry: &TabStripGeometry,
    anchor: TabOffsetAnchor,
) -> Option<IndexedTabOffsetAnchor> {
    match anchor {
        TabOffsetAnchor::Zero => Some(IndexedTabOffsetAnchor::Zero),
        TabOffsetAnchor::Left(widget_ptr) => {
            tab_index(geometry, widget_ptr).map(IndexedTabOffsetAnchor::Left)
        }
        TabOffsetAnchor::Right(widget_ptr) => {
            tab_index(geometry, widget_ptr).map(IndexedTabOffsetAnchor::Right)
        }
    }
}

fn rightmost_replay_seed_index(geometry: &TabStripGeometry) -> Option<usize> {
    geometry
        .tabs
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let offset =
                apply_selected_tab_offset_at_index(geometry, AnchoredTabOffset::default(), index)
                    .offset;
            (index, offset)
        })
        .min_by_key(|(_, offset)| *offset)
        .map(|(index, _)| index)
}

#[cfg(test)]
fn rightmost_replay_seed(geometry: &TabStripGeometry) -> Option<usize> {
    rightmost_replay_seed_index(geometry).map(|index| geometry.tabs[index].widget_ptr)
}

fn replay_from_zero_indexed<F>(
    geometry: &TabStripGeometry,
    requested_anchor: IndexedTabOffsetAnchor,
    selected_index: usize,
    rightmost_seed_index: Option<usize>,
    mut select: F,
) -> AnchoredTabOffset
where
    F: FnMut(usize),
{
    let mut anchored_offset = AnchoredTabOffset::default();
    let mut apply_selection = |index: usize| {
        select(geometry.tabs[index].widget_ptr);
        anchored_offset = apply_selected_tab_offset_at_index(geometry, anchored_offset, index);
    };

    match requested_anchor {
        IndexedTabOffsetAnchor::Zero => {}
        IndexedTabOffsetAnchor::Right(index) => apply_selection(index),
        IndexedTabOffsetAnchor::Left(index) => {
            // Starting from zero cannot recreate a left-edge anchor for a
            // naturally right-side tab. First create the furthest right-side
            // viewport FLTK can reach from zero, then reveal the anchored tab.
            if let Some(seed_index) = rightmost_seed_index {
                apply_selection(seed_index);
            }
            apply_selection(index);
        }
    }

    apply_selection(selected_index);
    anchored_offset
}

fn closest_replay_anchor(
    geometry: &TabStripGeometry,
    previous_offset: i32,
    selected_ptr: usize,
) -> TabOffsetAnchor {
    let Some(selected_index) = tab_index(geometry, selected_ptr) else {
        return TabOffsetAnchor::Zero;
    };
    let rightmost_seed_index = rightmost_replay_seed_index(geometry);
    let mut best_anchor = TabOffsetAnchor::Zero;
    let mut best_key = (i64::MAX, true);
    let mut consider = |indexed_anchor: IndexedTabOffsetAnchor, anchor: TabOffsetAnchor| {
        let replayed = replay_from_zero_indexed(
            geometry,
            indexed_anchor,
            selected_index,
            rightmost_seed_index,
            |_| {},
        );
        let key = (
            (i64::from(replayed.offset) - i64::from(previous_offset)).abs(),
            replayed.offset > previous_offset,
        );
        if key < best_key {
            best_key = key;
            best_anchor = anchor;
        }
    };

    consider(IndexedTabOffsetAnchor::Zero, TabOffsetAnchor::Zero);
    for (index, tab) in geometry.tabs.iter().enumerate() {
        consider(
            IndexedTabOffsetAnchor::Right(index),
            TabOffsetAnchor::Right(tab.widget_ptr),
        );
    }
    for (index, tab) in geometry.tabs.iter().enumerate() {
        consider(
            IndexedTabOffsetAnchor::Left(index),
            TabOffsetAnchor::Left(tab.widget_ptr),
        );
    }
    best_anchor
}

fn replay_tab_offset_anchor<F>(
    geometry: &TabStripGeometry,
    previous: AnchoredTabOffset,
    anchor_invalidated: bool,
    preserve_missing_offset: bool,
    selected_ptr: Option<usize>,
    select: F,
) -> AnchoredTabOffset
where
    F: FnMut(usize),
{
    let Some((selected_ptr, selected_index)) =
        selected_ptr.and_then(|ptr| tab_index(geometry, ptr).map(|index| (ptr, index)))
    else {
        return AnchoredTabOffset::default();
    };

    let previous_indexed = indexed_tab_offset_anchor(geometry, previous.anchor);
    let anchor_survives = previous.anchor == TabOffsetAnchor::Zero
        || (!anchor_invalidated && previous_indexed.is_some());
    let requested_anchor = if anchor_survives {
        previous.anchor
    } else if preserve_missing_offset {
        closest_replay_anchor(geometry, previous.offset, selected_ptr)
    } else {
        TabOffsetAnchor::Zero
    };
    let indexed_anchor = indexed_tab_offset_anchor(geometry, requested_anchor)
        .unwrap_or(IndexedTabOffsetAnchor::Zero);
    replay_from_zero_indexed(
        geometry,
        indexed_anchor,
        selected_index,
        rightmost_replay_seed_index(geometry),
        select,
    )
}

fn geometries_share_surviving_tab(
    previous: Option<&TabStripGeometry>,
    current: &TabStripGeometry,
    removed_widget_ptrs: &HashSet<usize>,
) -> bool {
    let current_widget_ptrs = current
        .tabs
        .iter()
        .map(|tab| tab.widget_ptr)
        .collect::<HashSet<_>>();
    previous.is_some_and(|previous| {
        previous.tabs.iter().any(|tab| {
            !removed_widget_ptrs.contains(&tab.widget_ptr)
                && current_widget_ptrs.contains(&tab.widget_ptr)
        })
    })
}

fn anchor_was_removed(anchor: TabOffsetAnchor, removed_widget_ptrs: &HashSet<usize>) -> bool {
    match anchor {
        TabOffsetAnchor::Zero => false,
        TabOffsetAnchor::Left(widget_ptr) | TabOffsetAnchor::Right(widget_ptr) => {
            removed_widget_ptrs.contains(&widget_ptr)
        }
    }
}

fn sync_overflow_mode_impl(
    tabs: &mut Tabs,
    state: &mut TabStripState,
    fallback_header_height: i32,
    force_pulldown_offset_reset: bool,
) -> bool {
    let Some(geometry) = tab_strip_geometry(tabs, fallback_header_height) else {
        return false;
    };
    let selected_ptr = tabs
        .value()
        .map(|selected| selected.as_widget_ptr() as usize);
    let use_pulldown = should_use_stable_pulldown(&geometry, selected_ptr);
    let geometry_changed =
        state.geometry.as_ref() != Some(&geometry) || !state.removed_widget_ptrs.is_empty();
    let preserve_missing_offset = geometries_share_surviving_tab(
        state.geometry.as_ref(),
        &geometry,
        &state.removed_widget_ptrs,
    );
    let previous = state.anchored_offset;
    let anchor_invalidated = anchor_was_removed(previous.anchor, &state.removed_widget_ptrs);
    let overflow_mode = overflow_sync_mode(
        state.pulldown_active,
        use_pulldown,
        geometry_changed || (use_pulldown && force_pulldown_offset_reset),
    );

    state.pulldown_active = use_pulldown;

    let Some(overflow_mode) = overflow_mode else {
        if !use_pulldown {
            state.anchored_offset = AnchoredTabOffset::default();
        }
        state.geometry = Some(geometry);
        state.removed_widget_ptrs.clear();
        return false;
    };

    if use_pulldown {
        let widgets = tabs
            .clone()
            .into_iter()
            .filter_map(|widget| widget.as_group())
            .filter(|group| !group.was_deleted())
            .map(|group| (group.as_widget_ptr() as usize, group))
            .collect::<Vec<_>>();

        // Fl_Tabs::handle_overflow() always clears tab_offset. Recreate the
        // previous left/right tab-edge anchor before restoring the selected
        // tab, so metric-only changes cannot re-anchor the whole strip from
        // zero. If a removed tab owned the anchor, preserve the closest
        // natively reproducible offset when another old tab proves this is the
        // same strip rather than a complete replacement.
        tabs.handle_overflow(overflow_mode);
        state.anchored_offset = replay_tab_offset_anchor(
            &geometry,
            previous,
            anchor_invalidated,
            preserve_missing_offset,
            selected_ptr,
            |widget_ptr| {
                if let Some((_, widget)) = widgets.iter().find(|(ptr, _)| *ptr == widget_ptr) {
                    let _ = tabs.set_value(widget);
                }
            },
        );
    } else {
        tabs.handle_overflow(overflow_mode);
        state.anchored_offset = AnchoredTabOffset::default();
    }
    state.geometry = Some(geometry);
    state.removed_widget_ptrs.clear();
    true
}

/// Dresses a strip in the connection tag its selected tab belongs to, or
/// restores the plain selection colours when there is no tag.
///
/// `Fl_Tabs::draw_tab` paints the selected tab with the strip's
/// `selection_color` and `labelcolor` and ignores the tab's own, so a tag can
/// only reach the selected tab from here. Every strip that shows a tag goes
/// through this, or two strips end up disagreeing about the same connection.
pub(crate) fn apply_tag_surface(tabs: &mut Tabs, tag: Option<Color>) {
    if tabs.was_deleted() {
        return;
    }
    if let Some(tag) = tag {
        tabs.set_selection_color(theme::tag_selected_surface(tag));
        tabs.set_label_color(theme::text_primary());
    } else {
        tabs.set_selection_color(theme::selection_soft());
        tabs.set_label_color(theme::text_secondary());
    }
}

pub(crate) fn record_removed_tab(state: &mut TabStripState, widget_ptr: usize) {
    state.removed_widget_ptrs.insert(widget_ptr);
}

pub(crate) fn record_selected_tab(
    tabs: &mut Tabs,
    state: &mut TabStripState,
    fallback_header_height: i32,
) {
    let Some(geometry) = tab_strip_geometry(tabs, fallback_header_height) else {
        return;
    };
    let selected_ptr = tabs
        .value()
        .map(|selected| selected.as_widget_ptr() as usize);
    let use_pulldown = should_use_stable_pulldown(&geometry, selected_ptr);
    if state.geometry.as_ref() != Some(&geometry) || state.pulldown_active != use_pulldown {
        sync_overflow_mode_impl(tabs, state, fallback_header_height, false);
        return;
    }
    if !use_pulldown {
        state.anchored_offset = AnchoredTabOffset::default();
        return;
    }
    if let Some(selected_ptr) = selected_ptr {
        state.anchored_offset =
            apply_selected_tab_offset(&geometry, state.anchored_offset, selected_ptr);
    }
}

pub(crate) fn sync_overflow_mode(
    tabs: &mut Tabs,
    state: &mut TabStripState,
    fallback_header_height: i32,
) -> bool {
    sync_overflow_mode_impl(tabs, state, fallback_header_height, false)
}

pub(crate) fn handle_pointer_event(
    tabs: &mut Tabs,
    ev: Event,
    state: &mut TabStripState,
    gesture: &mut TabStripPointerGesture,
    fallback_header_height: i32,
) -> bool {
    if ev == Event::MouseWheel && should_consume_mouse_wheel_for_tabs(tabs, fallback_header_height)
    {
        // Normally this handler runs before FLTK and consumes the wheel. During
        // a header press FLTK temporarily runs first so it can preserve native
        // click/close behavior; undo any wheel offset in that short interval.
        if gesture.active {
            sync_overflow_mode_impl(tabs, state, fallback_header_height, true);
        } else {
            // Also closes the small interval after a screen-scale/font metric
            // change but before its deferred or resize-driven refresh.
            sync_overflow_mode_impl(tabs, state, fallback_header_height, false);
        }
        return true;
    }

    match ev {
        Event::Push => {
            let header_height = tab_header_height(tabs, fallback_header_height);
            let event_x = app::event_x();
            let event_y = app::event_y();
            if !point_is_in_tab_header(
                tabs.x(),
                tabs.y(),
                tabs.w(),
                tabs.h(),
                header_height,
                event_x,
                event_y,
            ) {
                return false;
            }

            // Re-evaluate overflow at the interaction boundary as a final
            // safeguard for display-scale changes whose resize notification is
            // delivered after the input event.
            sync_overflow_mode_impl(tabs, state, fallback_header_height, false);
            if !state.pulldown_active
                || point_is_in_pulldown_button(tabs.x(), tabs.w(), header_height, event_x)
            {
                return false;
            }

            gesture.active = true;
            gesture.dragged = false;

            // This Push is still dispatched to FLTK after the current
            // pre-handler returns false. Subsequent Drag/Release events run
            // FLTK first, allowing us to restore its offset in the same
            // event without interfering with tab selection or close clicks.
            tabs.super_handle_first(true);
            false
        }
        Event::Drag if gesture.active => {
            gesture.dragged = true;
            sync_overflow_mode_impl(tabs, state, fallback_header_height, true);
            true
        }
        Event::Released if gesture.active => {
            if gesture.dragged {
                sync_overflow_mode_impl(tabs, state, fallback_header_height, true);
            }
            *gesture = TabStripPointerGesture::default();
            tabs.super_handle_first(false);
            true
        }
        Event::Unfocus | Event::Deactivate | Event::Hide if gesture.active => {
            if gesture.dragged {
                sync_overflow_mode_impl(tabs, state, fallback_header_height, true);
            }
            *gesture = TabStripPointerGesture::default();
            tabs.super_handle_first(false);
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        anchor_was_removed, apply_selected_tab_offset, geometries_share_surviving_tab,
        has_overflow, natural_available_width, overflow_sync_mode, point_is_in_pulldown_button,
        point_is_in_tab_header, record_removed_tab, replay_tab_offset_anchor,
        rightmost_replay_seed, selected_tab_can_stably_fit, shortcut_label_text,
        should_consume_mouse_wheel, should_use_pulldown, should_use_stable_pulldown, tab_width,
        try_with_state, AnchoredTabOffset, TabGeometry, TabOffsetAnchor, TabStripGeometry,
        TabStripState,
    };
    use fltk::group::TabsOverflow;
    use std::sync::Mutex;

    fn geometry(available_width: i32, tabs: &[(usize, i32)]) -> TabStripGeometry {
        let mut left = 0;
        let tab_geometries = tabs
            .iter()
            .map(|(widget_ptr, width)| {
                let tab = TabGeometry {
                    widget_ptr: *widget_ptr,
                    left,
                    width: *width,
                };
                left = left.saturating_add(*width);
                tab
            })
            .collect();
        TabStripGeometry {
            header_height: 25,
            tabs_width: available_width,
            available_width,
            total_width: tabs.iter().map(|(_, width)| *width).sum(),
            tabs: tab_geometries,
        }
    }

    fn replay(
        geometry: &TabStripGeometry,
        previous: AnchoredTabOffset,
        selected_ptr: usize,
    ) -> AnchoredTabOffset {
        replay_tab_offset_anchor(geometry, previous, false, true, Some(selected_ptr), |_| {})
    }

    fn replay_after_missing_anchor(
        geometry: &TabStripGeometry,
        previous: AnchoredTabOffset,
        preserve_missing_offset: bool,
        selected_ptr: usize,
    ) -> AnchoredTabOffset {
        replay_tab_offset_anchor(
            geometry,
            previous,
            true,
            preserve_missing_offset,
            Some(selected_ptr),
            |_| {},
        )
    }

    #[test]
    fn nested_tab_strip_state_access_is_skipped_instead_of_blocking() {
        let shared = Mutex::new(TabStripState::default());

        let nested = try_with_state(&shared, |_| try_with_state(&shared, |_| ()));

        assert_eq!(nested, Some(None));
        assert!(try_with_state(&shared, |_| ()).is_some());
    }

    #[test]
    fn wheel_is_consumed_everywhere_except_strictly_inside_content() {
        let args = (2, 10, 20, 320, 240, 25);

        assert!(should_consume_mouse_wheel(
            args.0, args.1, args.2, args.3, args.4, args.5, 10, 20
        ));
        assert!(should_consume_mouse_wheel(
            args.0, args.1, args.2, args.3, args.4, args.5, 329, 45
        ));
        assert!(!should_consume_mouse_wheel(
            args.0, args.1, args.2, args.3, args.4, args.5, 10, 46
        ));
        assert!(!should_consume_mouse_wheel(
            args.0, args.1, args.2, args.3, args.4, args.5, 329, 259
        ));
    }

    #[test]
    fn fallback_wheel_outside_tabs_is_always_consumed() {
        let args = (2, 10, 20, 320, 240, 25);

        assert!(should_consume_mouse_wheel(
            args.0, args.1, args.2, args.3, args.4, args.5, 9, 100
        ));
        assert!(should_consume_mouse_wheel(
            args.0, args.1, args.2, args.3, args.4, args.5, 330, 100
        ));
        assert!(should_consume_mouse_wheel(
            args.0, args.1, args.2, args.3, args.4, args.5, 100, -100
        ));
        assert!(should_consume_mouse_wheel(
            args.0, args.1, args.2, args.3, args.4, args.5, 100, 260
        ));
    }

    #[test]
    fn empty_or_degenerate_tabs_consume_wheel() {
        assert!(should_consume_mouse_wheel(0, 10, 20, 320, 240, 25, 10, 100));
        assert!(should_consume_mouse_wheel(1, 10, 20, 0, 240, 25, 10, 100));
        assert!(should_consume_mouse_wheel(1, 10, 20, 320, 0, 25, 10, 20));
        assert!(should_consume_mouse_wheel(1, 10, 20, 320, 10, 25, 10, 25));
    }

    #[test]
    fn wheel_policy_uses_current_logical_geometry_after_scale_resize() {
        assert!(!should_consume_mouse_wheel(
            2, 100, 80, 600, 400, 25, 699, 106
        ));
        assert!(should_consume_mouse_wheel(
            2, 50, 40, 300, 200, 25, 699, 106
        ));
        assert!(!should_consume_mouse_wheel(
            2, 50, 40, 300, 200, 25, 349, 66
        ));
    }

    #[test]
    fn header_and_pulldown_hit_regions_match_fltk_boundaries() {
        let args = (10, 20, 320, 240, 25);

        assert!(point_is_in_tab_header(
            args.0, args.1, args.2, args.3, args.4, 10, 20
        ));
        assert!(point_is_in_tab_header(
            args.0, args.1, args.2, args.3, args.4, 329, 45
        ));
        assert!(!point_is_in_tab_header(
            args.0, args.1, args.2, args.3, args.4, 330, 45
        ));
        assert!(!point_is_in_tab_header(
            args.0, args.1, args.2, args.3, args.4, 329, 46
        ));

        assert!(!point_is_in_pulldown_button(args.0, args.2, args.4, 306));
        assert!(point_is_in_pulldown_button(args.0, args.2, args.4, 307));
        assert!(point_is_in_pulldown_button(args.0, args.2, args.4, 329));
        assert!(!point_is_in_pulldown_button(args.0, args.2, args.4, 330));
    }

    #[test]
    fn closeable_tab_width_matches_fltk_extra_width() {
        assert_eq!(tab_width(80, 17, false), 90);
        assert_eq!(tab_width(80, 17, true), 100);
        assert_eq!(tab_width(80, 14, true), 99);
    }

    #[test]
    fn shortcut_markers_match_fltk_tab_label_processing() {
        assert_eq!(shortcut_label_text("A&B"), "AB");
        assert_eq!(shortcut_label_text("A&&B"), "A&B");
        assert_eq!(shortcut_label_text("A&"), "A&");
        assert_eq!(shortcut_label_text("&&&X"), "&X");
        assert_eq!(shortcut_label_text("한&글"), "한글");
    }

    #[test]
    fn overflow_threshold_is_strict() {
        assert!(!has_overflow(280, 280));
        assert!(has_overflow(281, 280));
        assert!(has_overflow(1, 0));
    }

    #[test]
    fn natural_tabs_do_not_enable_pulldown_to_reserve_a_button_that_is_not_needed() {
        let tabs_width = 500;

        assert!(!should_use_pulldown(
            2,
            tabs_width,
            natural_available_width(tabs_width, 0)
        ));
        assert!(should_use_pulldown(
            2,
            tabs_width + 1,
            natural_available_width(tabs_width, 0)
        ));
    }

    #[test]
    fn a_single_wide_tab_never_enables_scrollable_pulldown_mode() {
        assert!(!should_use_pulldown(0, 400, 280));
        assert!(!should_use_pulldown(1, 400, 280));
        assert!(should_use_pulldown(2, 400, 280));
    }

    #[test]
    fn selected_tab_fit_boundary_matches_fltk_value_margins() {
        let interior_fits = geometry(161, &[(1, 100), (2, 100), (3, 100), (4, 100), (5, 100)]);
        let interior_does_not_fit =
            geometry(160, &[(1, 100), (2, 100), (3, 100), (4, 100), (5, 100)]);
        let edge_fits = geometry(125, &[(1, 100), (2, 100)]);
        let edge_does_not_fit = geometry(124, &[(1, 100), (2, 100)]);

        assert!(selected_tab_can_stably_fit(&interior_fits, 3));
        assert!(!selected_tab_can_stably_fit(&interior_does_not_fit, 3));
        assert!(selected_tab_can_stably_fit(&edge_fits, 1));
        assert!(!selected_tab_can_stably_fit(&edge_does_not_fit, 1));
    }

    #[test]
    fn pulldown_requires_both_natural_overflow_and_a_stable_selected_tab() {
        let stable_overflow = geometry(161, &[(1, 100), (2, 100), (3, 100), (4, 100), (5, 100)]);
        let unstable_overflow = geometry(160, &[(1, 100), (2, 100), (3, 100), (4, 100), (5, 100)]);
        let no_overflow = geometry(600, &[(1, 100), (2, 100), (3, 100), (4, 100), (5, 100)]);

        assert!(should_use_stable_pulldown(&stable_overflow, Some(3)));
        assert!(!should_use_stable_pulldown(&unstable_overflow, Some(3)));
        assert!(!should_use_stable_pulldown(&no_overflow, Some(3)));
        assert!(!should_use_stable_pulldown(&stable_overflow, None));
    }

    #[test]
    fn selecting_an_unstable_tab_switches_to_compress_and_back() {
        let mixed_widths = geometry(180, &[(1, 80), (2, 160), (3, 80)]);

        assert!(should_use_stable_pulldown(&mixed_widths, Some(1)));
        assert!(!should_use_stable_pulldown(&mixed_widths, Some(2)));
        assert!(should_use_stable_pulldown(&mixed_widths, Some(3)));
        assert_eq!(
            overflow_sync_mode(true, false, false),
            Some(TabsOverflow::Compress)
        );
        assert_eq!(
            overflow_sync_mode(false, true, false),
            Some(TabsOverflow::Pulldown)
        );
    }

    #[test]
    fn overflow_mode_is_reapplied_when_overflow_geometry_changes() {
        assert_eq!(overflow_sync_mode(false, false, true), None);
        assert_eq!(overflow_sync_mode(true, true, false), None);
        assert_eq!(
            overflow_sync_mode(true, true, true),
            Some(TabsOverflow::Pulldown)
        );
        assert_eq!(
            overflow_sync_mode(false, true, true),
            Some(TabsOverflow::Pulldown)
        );
        assert_eq!(
            overflow_sync_mode(true, false, true),
            Some(TabsOverflow::Compress)
        );
    }

    #[test]
    fn scale_resize_crosses_overflow_threshold_in_both_directions() {
        let total_tab_width = 280;
        let wide = should_use_pulldown(2, total_tab_width, natural_available_width(360, 0));
        let narrow = should_use_pulldown(2, total_tab_width, natural_available_width(260, 0));

        assert!(!wide);
        assert!(narrow);
        assert_eq!(
            overflow_sync_mode(wide, narrow, true),
            Some(TabsOverflow::Pulldown)
        );
        assert_eq!(
            overflow_sync_mode(narrow, wide, true),
            Some(TabsOverflow::Compress)
        );
    }

    #[test]
    fn scale_resize_reapplies_pulldown_even_when_both_sizes_overflow() {
        let before = geometry(300, &[(1, 200), (2, 200)]);
        let after = geometry(260, &[(1, 200), (2, 200)]);

        assert!(before.is_overflowing());
        assert!(after.is_overflowing());
        assert_ne!(before, after);
        assert_eq!(
            overflow_sync_mode(true, true, before != after),
            Some(TabsOverflow::Pulldown)
        );
    }

    #[test]
    fn equal_total_width_with_changed_tab_positions_is_a_geometry_change() {
        let before = geometry(300, &[(1, 120), (2, 280)]);
        let label_changed = geometry(300, &[(1, 180), (2, 220)]);
        let first_tab_closed = geometry(300, &[(2, 280), (3, 120)]);

        assert_eq!(before.total_width, label_changed.total_width);
        assert_eq!(before.total_width, first_tab_closed.total_width);
        assert_ne!(before, label_changed);
        assert_ne!(before, first_tab_closed);
    }

    #[test]
    fn metric_change_preserves_existing_left_edge_anchor_instead_of_reanchoring_from_zero() {
        let before = geometry(300, &[(1, 100), (2, 100), (3, 100), (4, 100), (5, 100)]);
        let last_selected = apply_selected_tab_offset(&before, AnchoredTabOffset::default(), 5);
        let middle_selected = apply_selected_tab_offset(&before, last_selected, 3);
        assert_eq!(
            middle_selected,
            AnchoredTabOffset {
                anchor: TabOffsetAnchor::Left(3),
                offset: -180,
            }
        );

        // A background result tab crosses a row-count digit boundary. Its
        // width changes, but the selected tab's left edge and viewport anchor
        // do not. Replaying from zero with only the selected tab would jump
        // the strip from -180 to -41.
        let after = geometry(300, &[(1, 100), (2, 100), (3, 100), (4, 100), (5, 110)]);
        let preserved = replay(&after, middle_selected, 3);
        let zero_reanchored = apply_selected_tab_offset(&after, AnchoredTabOffset::default(), 3);

        assert_eq!(preserved, middle_selected);
        assert_eq!(zero_reanchored.offset, -41);
        assert_ne!(preserved.offset, zero_reanchored.offset);
    }

    #[test]
    fn metric_change_updates_only_the_edge_that_is_actually_anchored() {
        let before = geometry(300, &[(1, 100), (2, 100), (3, 100), (4, 100), (5, 100)]);
        let last_selected = apply_selected_tab_offset(&before, AnchoredTabOffset::default(), 5);
        assert_eq!(last_selected.anchor, TabOffsetAnchor::Right(5));
        assert_eq!(last_selected.offset, -223);

        let after = geometry(300, &[(1, 100), (2, 100), (3, 100), (4, 100), (5, 110)]);
        let replayed = replay(&after, last_selected, 5);

        // The last tab grew by ten pixels, so preserving its right edge moves
        // the strip by exactly ten pixels rather than rebuilding another view.
        assert_eq!(replayed.anchor, TabOffsetAnchor::Right(5));
        assert_eq!(replayed.offset, -233);
    }

    #[test]
    fn scale_resize_uses_compress_before_selected_tab_alignment_can_oscillate() {
        let initial = geometry(300, &[(1, 100), (2, 100), (3, 100), (4, 100), (5, 100)]);
        let last_selected = apply_selected_tab_offset(&initial, AnchoredTabOffset::default(), 5);
        let middle_selected = apply_selected_tab_offset(&initial, last_selected, 3);

        let wider = geometry(320, &[(1, 100), (2, 100), (3, 100), (4, 100), (5, 100)]);
        assert!(should_use_stable_pulldown(&wider, Some(3)));
        assert_eq!(replay(&wider, middle_selected, 3), middle_selected);

        let too_narrow = geometry(130, &[(1, 100), (2, 100), (3, 100), (4, 100), (5, 100)]);
        let right_aligned = apply_selected_tab_offset(&too_narrow, middle_selected, 3);
        let left_aligned = apply_selected_tab_offset(&too_narrow, right_aligned, 3);

        assert_eq!(right_aligned.anchor, TabOffsetAnchor::Right(3));
        assert_eq!(left_aligned.anchor, TabOffsetAnchor::Left(3));
        assert_eq!(left_aligned.offset, middle_selected.offset);
        assert!(!should_use_stable_pulldown(&too_narrow, Some(3)));
        assert_eq!(
            overflow_sync_mode(true, false, true),
            Some(TabsOverflow::Compress)
        );
    }

    #[test]
    fn closing_selected_anchor_preserves_the_closest_reproducible_offset() {
        let after_close = geometry(300, &[(1, 100), (2, 100), (4, 100), (5, 100)]);
        let previous = AnchoredTabOffset {
            anchor: TabOffsetAnchor::Left(3),
            offset: -180,
        };

        let replayed = replay_after_missing_anchor(&after_close, previous, true, 4);

        assert_eq!(replayed.anchor, TabOffsetAnchor::Right(5));
        assert_eq!(replayed.offset, -123);
        assert_eq!((replayed.offset - previous.offset).abs(), 57);
    }

    #[test]
    fn closing_background_anchor_can_preserve_the_exact_offset() {
        let after_close = geometry(300, &[(1, 100), (2, 100), (4, 100), (5, 100)]);
        let previous = AnchoredTabOffset {
            anchor: TabOffsetAnchor::Right(3),
            offset: -41,
        };

        let replayed = replay_after_missing_anchor(&after_close, previous, true, 2);

        assert_eq!(replayed.anchor, TabOffsetAnchor::Right(4));
        assert_eq!(replayed.offset, previous.offset);
    }

    #[test]
    fn equidistant_missing_anchor_avoids_the_rightward_candidate() {
        let after_close = geometry(300, &[(1, 100), (2, 100), (3, 101)]);
        let previous = AnchoredTabOffset {
            anchor: TabOffsetAnchor::Right(9),
            offset: -12,
        };

        let replayed = replay_after_missing_anchor(&after_close, previous, true, 2);

        assert_eq!(replayed.anchor, TabOffsetAnchor::Right(3));
        assert_eq!(replayed.offset, -24);
    }

    #[test]
    fn left_anchor_replay_uses_the_actual_furthest_right_seed() {
        let narrow_last_tab = geometry(100, &[(1, 100), (2, 100), (3, 12)]);

        // The wider interior tab needs more right margin than the last tab,
        // so selecting the last tab is not always the furthest reachable
        // viewport.
        assert_eq!(rightmost_replay_seed(&narrow_last_tab), Some(2));
    }

    #[test]
    fn removed_anchor_pointer_reuse_does_not_impersonate_the_old_tab() {
        let after_reuse = geometry(300, &[(3, 100), (1, 100), (2, 100), (4, 100), (5, 100)]);
        let previous = AnchoredTabOffset {
            anchor: TabOffsetAnchor::Left(3),
            offset: -180,
        };

        let replayed = replay_after_missing_anchor(&after_reuse, previous, true, 4);

        assert_eq!(replayed.anchor, TabOffsetAnchor::Left(2));
        assert_eq!(replayed.offset, previous.offset);
        assert_ne!(replayed, replay(&after_reuse, previous, 4));
    }

    #[test]
    fn complete_tab_replacement_rebuilds_from_selection_instead_of_old_offset() {
        let replacement = geometry(300, &[(6, 100), (7, 100), (8, 100), (9, 100), (10, 100)]);
        let previous = AnchoredTabOffset {
            anchor: TabOffsetAnchor::Left(3),
            offset: -180,
        };

        let replayed = replay_after_missing_anchor(&replacement, previous, false, 8);

        assert_eq!(replayed.anchor, TabOffsetAnchor::Right(8));
        assert_eq!(replayed.offset, -41);
    }

    #[test]
    fn removal_record_identifies_only_the_matching_anchor_and_deduplicates() {
        let mut state = TabStripState {
            anchored_offset: AnchoredTabOffset {
                anchor: TabOffsetAnchor::Left(3),
                offset: -180,
            },
            ..TabStripState::default()
        };

        record_removed_tab(&mut state, 2);
        assert!(!anchor_was_removed(
            state.anchored_offset.anchor,
            &state.removed_widget_ptrs
        ));
        record_removed_tab(&mut state, 3);
        record_removed_tab(&mut state, 3);

        assert!(anchor_was_removed(
            state.anchored_offset.anchor,
            &state.removed_widget_ptrs
        ));
        assert_eq!(state.removed_widget_ptrs.len(), 2);
        assert!(state.removed_widget_ptrs.contains(&2));
        assert!(state.removed_widget_ptrs.contains(&3));
    }

    #[test]
    fn structural_continuity_excludes_removed_pointers_that_get_reused() {
        let previous = geometry(300, &[(1, 100), (2, 100), (3, 100)]);
        let one_survivor = geometry(300, &[(2, 100), (3, 100), (4, 100)]);
        let reused_only = geometry(300, &[(3, 100), (4, 100), (5, 100)]);

        assert!(geometries_share_surviving_tab(
            Some(&previous),
            &one_survivor,
            &HashSet::from([1, 3])
        ));
        assert!(!geometries_share_surviving_tab(
            Some(&previous),
            &reused_only,
            &HashSet::from([1, 2, 3])
        ));
    }

    #[test]
    fn anchor_replay_is_idempotent_across_metric_and_scale_changes() {
        let source = geometry(300, &[(1, 80), (2, 120), (3, 90), (4, 140), (5, 70)]);
        let targets = [
            geometry(220, &[(1, 90), (2, 120), (3, 100), (4, 140), (5, 80)]),
            geometry(300, &[(1, 80), (2, 130), (3, 90), (4, 150), (5, 70)]),
            geometry(420, &[(1, 80), (2, 120), (3, 90), (4, 140), (5, 70)]),
        ];

        let mut reachable = vec![AnchoredTabOffset::default()];
        for first in 1..=5 {
            let first_offset =
                apply_selected_tab_offset(&source, AnchoredTabOffset::default(), first);
            reachable.push(first_offset);
            for second in 1..=5 {
                reachable.push(apply_selected_tab_offset(&source, first_offset, second));
            }
        }

        for target in &targets {
            for previous in &reachable {
                for selected in 1..=5 {
                    let once = replay(target, *previous, selected);
                    let twice = replay(target, once, selected);
                    assert_eq!(
                        once, twice,
                        "replay drifted: previous={previous:?}, selected={selected}, target={target:?}"
                    );
                }
            }
        }
    }
}
