#![allow(clippy::cargo, clippy::pedantic)]

// Minimal IME composition probe for the macOS first-syllable decomposition
// issue: a bare FLTK `TextEditor` with no custom handlers, no intellisense,
// no undo system and no syntax highlighting.
//
// Confirmed mechanism (trace of typing 장영환): after every input-source
// switch to Korean, the FIRST Hangul-composing keystroke bypasses the IME
// entirely — the raw jamo is committed as plain text (compose_state stays 0,
// nothing gets marked) and the IME starts composing only from the second
// keystroke, so the first syllable ends up decomposed ("ㅈㅏㅇ영환").
// Same deterministic bug is reported against ghostty (#12541); iTerm2 and
// Terminal.app are unaffected.
//
// Workaround experiment: watch the current keyboard input source and, when it
// changes, re-establish the view's NSTextInputContext before the next key.
//
// Usage:
//   cargo run --bin verify_ime_minimal            # trace only (known broken)
//   cargo run --bin verify_ime_minimal -- --cycle   # + ctx deactivate/activate on source switch (found ineffective)
//   cargo run --bin verify_ime_minimal -- --refocus # + makeFirstResponder cycle on source switch (found ineffective)
//   cargo run --bin verify_ime_minimal -- --repair  # + detect-and-merge repair (hangul_repair module)
//
// Type Hangul right after switching 한/영 and watch stderr: the first key
// must show compose_state>0 with a selection for the fix to be effective.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("verify_ime_minimal is macOS-only");
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;
    use std::os::raw::{c_char, c_long};

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_msgSend(receiver: *mut c_void, sel: *const c_void, ...) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *const c_void;
    }

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn TISCopyCurrentKeyboardInputSource() -> *mut c_void;
        fn TISGetInputSourceProperty(source: *mut c_void, key: *const c_void) -> *mut c_void;
        static kTISPropertyInputSourceID: *const c_void;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringGetCString(
            string: *const c_void,
            buffer: *mut c_char,
            buffer_size: c_long,
            encoding: u32,
        ) -> bool;
        fn CFRelease(cf: *const c_void);
    }

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

    unsafe fn msg0(receiver: *mut c_void, name: &str) -> *mut c_void {
        let Ok(name) = std::ffi::CString::new(name) else {
            return std::ptr::null_mut();
        };
        // SAFETY: The caller supplies a live Objective-C receiver and every
        // selector passed to this zero-argument helper has that signature.
        unsafe { objc_msgSend(receiver, sel_registerName(name.as_ptr())) }
    }

    unsafe fn msg1(receiver: *mut c_void, name: &str, arg: *mut c_void) -> *mut c_void {
        let Ok(name) = std::ffi::CString::new(name) else {
            return std::ptr::null_mut();
        };
        // SAFETY: The caller supplies a live receiver and a valid object (or
        // null) argument for selectors with exactly one object parameter.
        unsafe { objc_msgSend(receiver, sel_registerName(name.as_ptr()), arg) }
    }

    pub fn current_input_source_id() -> String {
        // SAFETY: TIS returns a retained input-source object or null. The
        // property is borrowed from that live source, the output buffer is
        // writable for its declared length, and the retained source is
        // released exactly once after the conversion attempt.
        unsafe {
            let source = TISCopyCurrentKeyboardInputSource();
            if source.is_null() {
                return String::new();
            }
            let id_ref = TISGetInputSourceProperty(source, kTISPropertyInputSourceID);
            let mut buf = [0 as c_char; 256];
            let ok = !id_ref.is_null()
                && CFStringGetCString(
                    id_ref,
                    buf.as_mut_ptr(),
                    buf.len() as c_long,
                    K_CF_STRING_ENCODING_UTF8,
                );
            CFRelease(source);
            if !ok {
                return String::new();
            }
            std::ffi::CStr::from_ptr(buf.as_ptr())
                .to_string_lossy()
                .into_owned()
        }
    }

    /// `deactivate` + `activate` on the content view's NSTextInputContext.
    pub fn cycle_input_context(ns_window: *mut c_void) {
        // SAFETY: The caller obtains `ns_window` from FLTK on the macOS main
        // thread. Null intermediate Objective-C objects are checked before use,
        // and all messages match the helper signatures.
        unsafe {
            let view = msg0(ns_window, "contentView");
            if view.is_null() {
                return;
            }
            let ctx = msg0(view, "inputContext");
            if ctx.is_null() {
                eprintln!("[fix] inputContext is nil");
                return;
            }
            msg0(ctx, "deactivate");
            msg0(ctx, "activate");
        }
    }

    /// Drop and re-establish first-responder status of the content view.
    pub fn refocus_first_responder(ns_window: *mut c_void) {
        // SAFETY: The caller obtains `ns_window` from FLTK on the macOS main
        // thread. `view` is checked for null, and `makeFirstResponder:` accepts
        // either null or the live content-view object.
        unsafe {
            let view = msg0(ns_window, "contentView");
            if view.is_null() {
                return;
            }
            msg1(ns_window, "makeFirstResponder:", std::ptr::null_mut());
            msg1(ns_window, "makeFirstResponder:", view);
        }
    }
}

#[cfg(target_os = "macos")]
fn main() {
    use fltk::{
        app,
        enums::Event,
        prelude::*,
        text::{TextBuffer, TextEditor},
        window::Window,
    };

    let mode = std::env::args().nth(1).unwrap_or_default();
    let fltk_app = app::App::default();
    let mut window = Window::default()
        .with_size(640, 400)
        .with_label("IME minimal probe");
    let mut editor = TextEditor::default().with_size(620, 380).center_of_parent();
    let mut buffer = TextBuffer::default();
    editor.set_buffer(buffer.clone());
    window.end();
    window.show();

    buffer.add_modify_callback2(|buf, pos, inserted, deleted, _restyled, deleted_text| {
        if inserted <= 0 && deleted <= 0 {
            return;
        }
        eprintln!(
            "[modify] pos={pos} ins={inserted} del={deleted} deleted_text={deleted_text:?} \
             compose_state={} selection={:?}",
            app::compose_state(),
            buf.selection_position(),
        );
    });

    let repair_enabled = mode == "--repair";
    let mut repair_state =
        space_query::ui::sql_editor::hangul_repair::FirstKeyRepairState::default();
    let mut buffer_for_repair = buffer.clone();
    editor.handle(move |ed, event| {
        match event {
            Event::KeyDown | Event::KeyUp => {
                eprintln!(
                    "[{event:?}] key={:?} text={:?} compose_state={} caret={} selection={:?}",
                    app::event_key(),
                    app::event_text(),
                    app::compose_state(),
                    ed.insert_position(),
                    ed.buffer().and_then(|b| b.selection_position()),
                );
                if repair_enabled && event == Event::KeyDown {
                    let mods = app::event_state();
                    let has_command_modifiers = mods.contains(fltk::enums::Shortcut::Ctrl)
                        || mods.contains(fltk::enums::Shortcut::Command)
                        || mods.contains(fltk::enums::Shortcut::Alt);
                    let reader = buffer_for_repair.clone();
                    let edit = repair_state.on_key_event(
                        &app::event_text(),
                        has_command_modifiers,
                        app::compose_state().max(0) as usize,
                        ed.insert_position().max(0) as usize,
                        &|start, end| reader.text_range(start as i32, end as i32),
                    );
                    if let Some(edit) = edit {
                        eprintln!("[repair] merge {edit:?}");
                        buffer_for_repair.replace(
                            edit.start as i32,
                            edit.end as i32,
                            &edit.replacement,
                        );
                    }
                }
            }
            _ => {}
        }
        false
    });

    let _ = editor.take_focus();

    if mode == "--cycle" || mode == "--refocus" {
        let ns_window = window.raw_handle();
        let refocus = mode == "--refocus";
        let mut last_source = macos::current_input_source_id();
        eprintln!("[fix] watching input source (start: {last_source}), mode={mode}");
        app::add_timeout3(0.2, move |handle| {
            let source = macos::current_input_source_id();
            if source != last_source {
                eprintln!("[fix] input source changed: {last_source} -> {source}");
                if refocus {
                    macos::refocus_first_responder(ns_window as *mut _);
                } else {
                    macos::cycle_input_context(ns_window as *mut _);
                }
                last_source = source;
            }
            app::repeat_timeout3(0.2, handle);
        });
    }

    if let Err(err) = fltk_app.run() {
        eprintln!("fltk app loop failed: {err}");
    }
}
