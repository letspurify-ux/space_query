use crate::ui::sql_editor::HighlightShadowState;
use fltk::text::TextBuffer;
use std::sync::{Arc, Mutex};

fn with_current_shadow<R>(
    buffer: &TextBuffer,
    shadow: Option<&Arc<Mutex<HighlightShadowState>>>,
    f: impl FnOnce(&HighlightShadowState) -> Option<R>,
) -> Option<R> {
    let shadow = shadow?;
    let buffer_len = buffer.length().max(0) as usize;
    let guard = shadow
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.len() != buffer_len {
        return None;
    }
    f(&guard)
}

pub(crate) fn line_start(
    buffer: &TextBuffer,
    shadow: Option<&Arc<Mutex<HighlightShadowState>>>,
    pos: i32,
) -> i32 {
    let buffer_len = buffer.length().max(0);
    let clamped = pos.clamp(0, buffer_len) as usize;
    with_current_shadow(buffer, shadow, |shadow| {
        i32::try_from(shadow.line_start(clamped)).ok()
    })
    .unwrap_or_else(|| buffer.line_start(pos.clamp(0, buffer_len)))
}

pub(crate) fn line_end(
    buffer: &TextBuffer,
    shadow: Option<&Arc<Mutex<HighlightShadowState>>>,
    pos: i32,
) -> i32 {
    let buffer_len = buffer.length().max(0);
    let clamped = pos.clamp(0, buffer_len) as usize;
    with_current_shadow(buffer, shadow, |shadow| {
        i32::try_from(shadow.line_end(clamped)).ok()
    })
    .unwrap_or_else(|| buffer.line_end(pos.clamp(0, buffer_len)))
}

pub(crate) fn text_range(
    buffer: &TextBuffer,
    shadow: Option<&Arc<Mutex<HighlightShadowState>>>,
    start: i32,
    end: i32,
) -> String {
    let buffer_len = buffer.length().max(0);
    let clamped_start = start.clamp(0, buffer_len);
    let clamped_end = end.clamp(clamped_start, buffer_len);
    with_current_shadow(buffer, shadow, |shadow| {
        shadow.text_range_string(clamped_start as usize, clamped_end as usize)
    })
    .unwrap_or_else(|| {
        buffer
            .text_range(clamped_start, clamped_end)
            .unwrap_or_default()
    })
}

pub(crate) fn bounded_text_window(
    buffer: &TextBuffer,
    shadow: Option<&Arc<Mutex<HighlightShadowState>>>,
    start: i32,
    end: i32,
) -> (String, i32) {
    let buffer_len = buffer.length().max(0);
    let start = start.clamp(0, buffer_len);
    let end = end.clamp(start, buffer_len);
    if start >= end {
        return (String::new(), start);
    }

    if with_current_shadow(buffer, shadow, |shadow| {
        shadow.text_range_string(start as usize, end as usize)
    })
    .is_some()
    {
        return (text_range(buffer, shadow, start, end), start);
    }

    if let Some(text) = buffer.text_range(start, end) {
        return (text, start);
    }

    let fallback_start = line_start(buffer, shadow, start).max(0).min(end);
    let fallback_end = line_end(buffer, shadow, end)
        .max(fallback_start)
        .min(buffer_len);
    if fallback_start < fallback_end {
        return (
            text_range(buffer, shadow, fallback_start, fallback_end),
            fallback_start,
        );
    }

    (String::new(), start)
}
