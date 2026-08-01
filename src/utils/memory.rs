#[cfg(target_os = "linux")]
pub fn malloc_trim_process() {
    unsafe {
        // Request glibc allocator to return as many free pages as possible
        // back to the OS after large buffers are released.
        // SAFETY: FFI call has no Rust aliasing requirements and accepts any usize.
        libc::malloc_trim(0);
    }
}

#[cfg(target_os = "windows")]
pub fn malloc_trim_process() {
    unsafe {
        // Ask Windows to trim the current process working set so recently-freed
        // pages from large editor buffers are more likely to be returned to the OS.
        // SAFETY: Win32 APIs are called with the pseudo-handle for current process.
        let process = windows_sys::Win32::System::Threading::GetCurrentProcess();
        let _ = windows_sys::Win32::System::ProcessStatus::K32EmptyWorkingSet(process);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn malloc_trim_process() {}
