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

/// Number of lines in the buffer, never less than one.
pub(crate) fn line_count(
    buffer: &TextBuffer,
    shadow: Option<&Arc<Mutex<HighlightShadowState>>>,
) -> usize {
    with_current_shadow(buffer, shadow, |shadow| Some(shadow.line_count()))
        .unwrap_or_else(|| {
            let length = buffer.length().max(0);
            buffer.count_lines(0, length).max(0) as usize + 1
        })
        .max(1)
}

/// Zero-based line index the byte at `pos` sits on.
pub(crate) fn line_index_for_position(
    buffer: &TextBuffer,
    shadow: Option<&Arc<Mutex<HighlightShadowState>>>,
    pos: i32,
) -> usize {
    let buffer_len = buffer.length().max(0);
    let clamped = pos.clamp(0, buffer_len);
    with_current_shadow(buffer, shadow, |shadow| {
        Some(shadow.line_index_for_position(clamped as usize))
    })
    .unwrap_or_else(|| buffer.count_lines(0, clamped).max(0) as usize)
}

/// Byte offset the zero-based `line_index` starts at.
pub(crate) fn line_start_for_index(
    buffer: &TextBuffer,
    shadow: Option<&Arc<Mutex<HighlightShadowState>>>,
    line_index: usize,
) -> i32 {
    let buffer_len = buffer.length().max(0);
    with_current_shadow(buffer, shadow, |shadow| {
        i32::try_from(shadow.line_start_for_index(line_index)).ok()
    })
    .unwrap_or_else(|| {
        // FLTK has no "start of line N", so walk line ends. Only reached when
        // the shadow is out of step with the buffer, which is rare, and the
        // walk stops at the requested line rather than reading the document.
        let mut start = 0;
        for _ in 0..line_index {
            let next = buffer.line_end(start).saturating_add(1);
            if next > buffer_len {
                return buffer_len;
            }
            start = next;
        }
        start
    })
    .clamp(0, buffer_len)
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

    // Report the boundary-aligned start the slice actually begins at: a
    // mid-character window start (cursor - WINDOW landing inside a multibyte
    // char) is backed up to the previous UTF-8 boundary by the slice, so
    // returning the raw `start` desyncs callers' `abs = start + rel` math.
    if let Some((text, aligned_start)) = with_current_shadow(buffer, shadow, |shadow| {
        shadow.text_range_string_with_aligned_start(start as usize, end as usize)
    }) {
        return (text, aligned_start as i32);
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
