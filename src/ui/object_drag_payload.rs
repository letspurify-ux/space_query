const START_MARKER: &str = "__SPACE_QUERY_OBJECT_DRAG_V1__";
const END_MARKER: &str = "__SPACE_QUERY_OBJECT_DRAG_END__";
const ACTIVE_DRAG_TIMEOUT_SECONDS: f64 = 15.0;
const BLANK_DRAG_PREVIEW_TEXT: &str = "\n";

use crate::utils::arithmetic::{safe_div, safe_rem};
use fltk::app;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug)]
struct ActiveObjectDrag {
    generation: u64,
    display_text: String,
    insert_text: String,
}

static ACTIVE_OBJECT_DRAG: OnceLock<Mutex<Option<ActiveObjectDrag>>> = OnceLock::new();
static NEXT_ACTIVE_OBJECT_DRAG_GENERATION: AtomicU64 = AtomicU64::new(1);

fn active_object_drag_slot() -> &'static Mutex<Option<ActiveObjectDrag>> {
    ACTIVE_OBJECT_DRAG.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
pub(crate) fn encode(text: &str) -> String {
    format!(
        "{}\n{}\n{}",
        START_MARKER,
        encode_hex(text.as_bytes()),
        END_MARKER
    )
}

pub(crate) fn decode(raw: &str) -> Option<String> {
    let cleaned = raw.trim_matches('\0').trim();
    let mut lines = cleaned
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());

    if lines.next()? != START_MARKER {
        return None;
    }
    let encoded = lines.next()?;
    if lines.next()? != END_MARKER {
        return None;
    }
    if lines.next().is_some() {
        return None;
    }

    String::from_utf8(decode_hex(encoded)?).ok()
}

pub(crate) fn start_drag(insert_text: &str) {
    let generation = store_active_drag(insert_text);
    app::copy2(BLANK_DRAG_PREVIEW_TEXT);
    start_platform_dnd();
    crate::ui::ui_timeout::schedule(ACTIVE_DRAG_TIMEOUT_SECONDS, move || {
        clear_active_drag_generation(generation);
    });
}

pub(crate) fn take_active_drag_text(raw: &str) -> Option<String> {
    let display_text = normalize_drag_text(raw);
    let mut guard = active_object_drag_slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let active = guard.as_ref()?;
    if active.display_text != display_text {
        return None;
    }
    guard.take().map(|active| active.insert_text)
}

fn store_active_drag(insert_text: &str) -> u64 {
    let generation = NEXT_ACTIVE_OBJECT_DRAG_GENERATION.fetch_add(1, Ordering::Relaxed);
    let active = ActiveObjectDrag {
        generation,
        display_text: normalize_drag_text(BLANK_DRAG_PREVIEW_TEXT),
        insert_text: insert_text.to_string(),
    };
    *active_object_drag_slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(active);
    generation
}

fn clear_active_drag_generation(generation: u64) {
    let mut guard = active_object_drag_slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard
        .as_ref()
        .is_some_and(|active| active.generation == generation)
    {
        *guard = None;
    }
}

fn normalize_drag_text(value: &str) -> String {
    value.trim_matches('\0').trim().to_string()
}

#[cfg(target_os = "macos")]
fn start_platform_dnd() {
    use std::ffi::c_void;
    use std::os::raw::c_int;

    extern "C" {
        #[link_name = "_ZN2Fl13screen_driverEv"]
        fn fl_screen_driver() -> *mut c_void;
        #[link_name = "_ZN22Fl_Cocoa_Screen_Driver3dndEi"]
        fn fl_cocoa_screen_driver_dnd(driver: *mut c_void, use_selection: c_int) -> c_int;
    }

    // SAFETY: Both symbols are provided by the linked FLTK Cocoa backend.
    // `fl_screen_driver` returns either null or FLTK's live screen-driver
    // singleton; the latter is the required receiver for its `dnd` method.
    unsafe {
        let driver = fl_screen_driver();
        if driver.is_null() {
            app::dnd();
        } else {
            let _ = fl_cocoa_screen_driver_dnd(driver, 1);
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn start_platform_dnd() {
    app::dnd();
}

#[cfg(test)]
fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    if safe_rem(bytes.len(), 2) != 0 {
        return None;
    }

    let mut out = Vec::with_capacity(safe_div(bytes.len(), 2));
    for pair in bytes.chunks_exact(2) {
        let high = hex_value(pair[0])?;
        let low = hex_value(pair[1])?;
        out.push((high << 4) | low);
    }
    Some(out)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_drag_payload_round_trips_identifier_text() {
        let text = r#"SCOTT."EMP TABLE""#;
        let payload = encode(text);

        assert_eq!(decode(&payload), Some(text.to_string()));
    }

    #[test]
    fn object_drag_payload_ignores_plain_text() {
        assert_eq!(decode("file:///tmp/query.sql"), None);
        assert_eq!(decode("EMPLOYEES"), None);
    }

    #[test]
    fn object_drag_payload_rejects_malformed_hex() {
        let payload = format!("{}\nnot-hex\n{}", START_MARKER, END_MARKER);

        assert_eq!(decode(&payload), None);
    }

    #[test]
    fn object_drag_payload_rejects_trailing_content() {
        let payload = format!("{}\n{}\n{}\nextra", START_MARKER, "454d50", END_MARKER);

        assert_eq!(decode(&payload), None);
    }

    #[test]
    fn active_object_drag_text_is_consumed_when_blank_preview_text_matches() {
        store_active_drag(r#"SCOTT."EMP TABLE""#);

        assert_eq!(take_active_drag_text("SCOTT.\"EMP TABLE\""), None);
        assert_eq!(
            take_active_drag_text(BLANK_DRAG_PREVIEW_TEXT),
            Some(r#"SCOTT."EMP TABLE""#.to_string())
        );
        assert_eq!(take_active_drag_text(BLANK_DRAG_PREVIEW_TEXT), None);
    }
}
