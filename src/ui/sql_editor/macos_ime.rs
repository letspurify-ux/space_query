//! macOS NSTextInputContext helpers for keeping the IME in sync with FLTK.
//!
//! FLTK's `performKeyEquivalent:` handles Cmd/Ctrl shortcuts by zeroing
//! `Fl::compose_state` and dispatching FL_KEYBOARD directly — the event never
//! reaches `[[view inputContext] handleEvent:]`, so an in-progress Hangul
//! composition is dropped on the FLTK side while the IME still believes it is
//! composing. The IME then commits the stale syllable together with the next
//! keystroke (e.g. 홍길동 → Cmd+A → retype 홍길동 yields 동홍길동). Mouse
//! clicks bypass the input context the same way and additionally leave
//! `Fl::compose_state` stale. `discard_marked_text` tells the IME to drop its
//! pending composition; it is a no-op when nothing is being composed.

use std::ffi::{c_void, CString};

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_msgSend(receiver: *mut c_void, sel: *const c_void, ...) -> *mut c_void;
    fn sel_registerName(name: *const std::os::raw::c_char) -> *const c_void;
}

unsafe fn msg0(receiver: *mut c_void, name: &str) -> *mut c_void {
    let Ok(name) = CString::new(name) else {
        return std::ptr::null_mut();
    };
    // SAFETY: The caller supplies a live Objective-C receiver. `name` is a
    // NUL-terminated selector spelling that remains alive for registration,
    // and every selector used here takes no explicit arguments.
    unsafe { objc_msgSend(receiver, sel_registerName(name.as_ptr())) }
}

/// Ask the IME attached to `ns_window`'s content view to discard any pending
/// (marked) composition. Text already inserted into the buffer is unaffected.
pub(crate) fn discard_marked_text(ns_window: *mut c_void) {
    if ns_window.is_null() {
        return;
    }
    // SAFETY: FLTK provides `ns_window` as a live NSWindow pointer on the main
    // thread. Each zero-argument Objective-C message is guarded against a null
    // result before that result is used as the next receiver.
    unsafe {
        let view = msg0(ns_window, "contentView");
        if view.is_null() {
            return;
        }
        let ctx = msg0(view, "inputContext");
        if ctx.is_null() {
            return;
        }
        msg0(ctx, "discardMarkedText");
    }
}
