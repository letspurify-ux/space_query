//! Which directories this process is allowed to read and write.
//!
//! Persistence to the user's real configuration and data directories is off
//! until the application turns it on. Anything else — a `verify_*` harness
//! binary, a test, a capture run — works in a per-process scratch directory
//! instead.
//!
//! This is deliberately fail-closed. A harness binary that builds a
//! `MainWindow` drives the same code the application does, so a menu item that
//! persists a preference writes a whole `AppConfig`; when that config was built
//! from `AppConfig::default()` rather than loaded from disk, the write replaces
//! every saved connection with an empty list. Requiring an explicit opt-in
//! means a new harness binary is safe by omission, and only the application has
//! to remember anything.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

static USER_DATA_PERSISTENCE: AtomicBool = AtomicBool::new(false);

/// Let this process use the user's real configuration and data directories.
///
/// Called once by `src/main.rs`. Nothing else should call it: a harness binary
/// that needs to exercise persistence sets `SPACE_QUERY_CONFIG_DIR` and
/// `SPACE_QUERY_DATA_DIR` to a directory of its own instead.
pub fn enable_user_data_persistence() {
    USER_DATA_PERSISTENCE.store(true, Ordering::Release);
}

pub fn user_data_persistence_enabled() -> bool {
    USER_DATA_PERSISTENCE.load(Ordering::Acquire)
}

/// The directory a process that has not opted in writes to instead.
///
/// Per process, so two harness binaries running at once cannot read each
/// other's leftovers.
pub fn scratch_base_dir() -> PathBuf {
    std::env::temp_dir().join(format!("space_query-scratch-{}", std::process::id()))
}

/// The base directory for user data of the given kind, or the scratch
/// directory when this process has not opted in.
///
/// `real` is only consulted when persistence is enabled, so a process without
/// the opt-in never even resolves the user's directory.
pub fn base_dir_or_scratch(real: impl FnOnce() -> Option<PathBuf>) -> Option<PathBuf> {
    if user_data_persistence_enabled() {
        real()
    } else {
        Some(scratch_base_dir())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate is closed by default. A test binary never opts in, so this also
    /// proves that nothing in the test suite can reach the user's real
    /// directories: a `MainWindow` built here writes to the scratch directory.
    #[test]
    fn persistence_is_off_until_the_application_enables_it() {
        assert!(
            !user_data_persistence_enabled(),
            "a process that never called enable_user_data_persistence must not persist user data"
        );
        let resolved = base_dir_or_scratch(|| Some(PathBuf::from("/real/config")));
        assert_eq!(
            resolved,
            Some(scratch_base_dir()),
            "a process without the opt-in must be redirected to its scratch directory"
        );
    }

    /// Two harness binaries running at once must not share a directory, or one
    /// reads the other's half-written config.
    #[test]
    fn the_scratch_directory_is_per_process() {
        assert!(scratch_base_dir()
            .to_string_lossy()
            .ends_with(&std::process::id().to_string()));
    }
}
