use std::ffi::c_void;

#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct NativeWindowFrame {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

unsafe extern "C" {
    fn space_query_macos_capture_window_frame(
        raw_window: *mut c_void,
        out_frame: *mut NativeWindowFrame,
    ) -> i32;
    fn space_query_macos_restore_window_frame(
        raw_window: *mut c_void,
        frame: *const NativeWindowFrame,
    ) -> i32;
    fn space_query_macos_window_is_zoomed(raw_window: *mut c_void) -> i32;
    fn space_query_macos_set_window_zoomed(raw_window: *mut c_void, zoomed: i32) -> i32;
}

pub(crate) fn capture_frame(raw_window: *mut c_void) -> Option<NativeWindowFrame> {
    let mut frame = NativeWindowFrame {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
    };
    // SAFETY: FLTK supplies a live NSWindow pointer while the shown window is
    // being handled on the main thread. The output points to initialized,
    // writable storage for the duration of the call.
    let captured =
        unsafe { space_query_macos_capture_window_frame(raw_window, &mut frame as *mut _) };
    if captured != 0
        && frame.x.is_finite()
        && frame.y.is_finite()
        && frame.width.is_finite()
        && frame.height.is_finite()
        && frame.width > 0.0
        && frame.height > 0.0
    {
        Some(frame)
    } else {
        None
    }
}

pub(crate) fn restore_frame(raw_window: *mut c_void, frame: NativeWindowFrame) -> bool {
    // SAFETY: The frame was captured from this live NSWindow on the main
    // thread, and the immutable pointer remains valid for the call.
    unsafe { space_query_macos_restore_window_frame(raw_window, &frame as *const _) != 0 }
}

pub(crate) fn is_zoomed(raw_window: *mut c_void) -> bool {
    // SAFETY: The caller passes FLTK's live NSWindow handle on the main thread.
    unsafe { space_query_macos_window_is_zoomed(raw_window) != 0 }
}

pub(crate) fn set_zoomed(raw_window: *mut c_void, zoomed: bool) -> bool {
    // SAFETY: The caller passes FLTK's live NSWindow handle on the main thread.
    unsafe { space_query_macos_set_window_zoomed(raw_window, i32::from(zoomed)) != 0 }
}
