#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use space_query::app::App;
use space_query::utils::logging;

fn main() {
    let previous_panic_hook = std::panic::take_hook();

    // Install custom panic hook for crash handling
    std::panic::set_hook(Box::new(move |panic_info| {
        let location = panic_info
            .location()
            .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
            .unwrap_or_else(|| "unknown location".to_string());

        let payload = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic payload".to_string()
        };

        let crash_message = format!(
            "Panic at {}\nMessage: {}\n\nFull info: {}\n\nBacktrace:\n{}",
            location,
            payload,
            panic_info,
            std::backtrace::Backtrace::force_capture()
        );

        // NOTE: Avoid using the async logger here.
        // If panic happens while a log mutex is held, logging from the panic hook
        // can deadlock before the process exits.
        logging::write_crash_log(&crash_message);

        // Preserve default panic diagnostics (panic message, stack trace behavior, etc.).
        previous_panic_hook(panic_info);

        eprintln!("SPACE Query crashed: {}", crash_message);
    }));

    logging::log_info("app", "SPACE Query starting");

    let app = App::new();
    app.run();

    // Best-effort shutdown logging to reduce chances of losing the last entries
    // when the process exits right after UI teardown.
    logging::log_info("app", "SPACE Query stopping");
    if let Err(err) = logging::flush_log_writer() {
        eprintln!("Failed to flush application log before exit: {err}");
    }
}
