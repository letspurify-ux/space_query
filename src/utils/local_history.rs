//! Keep unsaved editor tabs across a crash.
//!
//! The app writes a snapshot of every tab holding unsaved text and deletes that
//! snapshot on the way out of a clean exit. So the file existing at startup
//! means exactly one thing — the last run did not shut down — and that is what
//! the restore prompt is keyed on. Someone who quits normally never sees any of
//! this.
//!
//! Nothing here decides *what* is unsaved. The editor already tracks that
//! against each tab's pristine text; this module only persists the answer.

use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const APP_DIR_NAME: &str = "space_query";
const UNSAVED_TABS_FILE_NAME: &str = "unsaved_tabs.json";

/// Largest tab this will carry.
///
/// A tab past this is skipped rather than truncated. Restoring a silently
/// shortened script is worse than not restoring it: the user would edit and
/// save a file that lost its tail without ever being told.
pub const MAX_SNAPSHOT_TAB_BYTES: usize = 8 * 1024 * 1024;

/// One editor tab's unsaved text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TabSnapshot {
    /// The tab's own name, so a never-saved tab is recognizable in the prompt.
    pub label: String,
    /// The file the text belongs to, when it has one.
    pub file_path: Option<PathBuf>,
    pub text: String,
}

impl TabSnapshot {
    /// What the restore prompt calls this tab.
    pub fn display_name(&self) -> String {
        self.file_path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| self.label.clone())
    }
}

/// Every unsaved tab from one run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionSnapshot {
    pub tabs: Vec<TabSnapshot>,
}

impl SessionSnapshot {
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Build a snapshot from the tabs an editor reports as dirty, dropping the
    /// ones too large to carry.
    ///
    /// Returns the snapshot and the names of any skipped tabs, so the caller can
    /// say so rather than leave the omission silent.
    pub fn from_dirty_tabs(tabs: Vec<TabSnapshot>) -> (Self, Vec<String>) {
        let mut skipped = Vec::new();
        let tabs = tabs
            .into_iter()
            .filter(|tab| {
                if tab.text.len() > MAX_SNAPSHOT_TAB_BYTES {
                    skipped.push(tab.display_name());
                    false
                } else {
                    true
                }
            })
            .collect();
        (Self { tabs }, skipped)
    }

    /// A cheap value that changes whenever the snapshot would.
    ///
    /// The autosave tick compares this against the last write instead of
    /// serializing every time: most ticks find nothing changed, and a large
    /// script should not be re-encoded once a second to discover that.
    pub fn change_key(&self) -> u64 {
        let mut key: u64 = 0xcbf2_9ce4_8422_2325;
        for tab in &self.tabs {
            for bytes in [
                tab.label.as_bytes(),
                tab.text.as_bytes(),
                tab.file_path
                    .as_ref()
                    .and_then(|path| path.to_str())
                    .unwrap_or("")
                    .as_bytes(),
            ] {
                key ^= bytes.len() as u64;
                key = key.wrapping_mul(0x0000_0100_0000_01b3);
                for chunk in bytes.chunks(64) {
                    for byte in chunk {
                        key ^= u64::from(*byte);
                        key = key.wrapping_mul(0x0000_0100_0000_01b3);
                    }
                }
            }
        }
        key
    }
}

pub fn snapshot_path() -> Option<PathBuf> {
    crate::utils::logging::app_data_base_dir().map(|mut path| {
        path.push(APP_DIR_NAME);
        path.push(UNSAVED_TABS_FILE_NAME);
        path
    })
}

/// Read the snapshot the last run left behind, if any.
///
/// A damaged file is discarded rather than preserved: unlike query history it
/// records nothing the user asked to keep, and a parse failure here must not
/// stop the app from starting.
pub fn load() -> Option<SessionSnapshot> {
    let path = snapshot_path()?;
    let content = fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<SessionSnapshot>(&content) {
        Ok(snapshot) if !snapshot.is_empty() => Some(snapshot),
        Ok(_) => None,
        Err(err) => {
            eprintln!(
                "Discarding unreadable unsaved-tab snapshot {}: {err}",
                path.display()
            );
            let _ = fs::remove_file(&path);
            None
        }
    }
}

pub fn save(snapshot: &SessionSnapshot) -> Result<(), String> {
    let path = snapshot_path().ok_or_else(|| "Local data directory is unavailable".to_string())?;
    if snapshot.is_empty() {
        // Nothing unsaved is the same state as never having crashed.
        return clear();
    }
    save_to_path(snapshot, &path)
}

/// Remove the snapshot. Called on a clean exit, which is what makes a surviving
/// snapshot mean "this run died".
pub fn clear() -> Result<(), String> {
    let Some(path) = snapshot_path() else {
        return Ok(());
    };
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

fn save_to_path(snapshot: &SessionSnapshot, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }

    // Write beside the target and rename: a crash during the write must not be
    // able to leave a half-written snapshot, which is the one moment this file
    // is about to be needed.
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let tmp_path = path.with_file_name(format!(
        "{}.tmp.{}.{}",
        UNSAVED_TABS_FILE_NAME,
        std::process::id(),
        unique
    ));

    let write_result = (|| -> Result<(), String> {
        let mut file = fs::File::create(&tmp_path).map_err(|err| err.to_string())?;
        serde_json::to_writer(&mut file, snapshot).map_err(|err| err.to_string())?;
        file.flush().map_err(|err| err.to_string())?;
        file.sync_all().map_err(|err| err.to_string())
    })();

    if let Err(err) = write_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    // Unsaved SQL can hold credentials and literals; keep it owner-only, the
    // same way query history is kept.
    #[cfg(unix)]
    if let Err(err) = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600)) {
        eprintln!("Warning: could not set unsaved-tab snapshot permissions: {err}");
    }

    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(label: &str, text: &str) -> TabSnapshot {
        TabSnapshot {
            label: label.to_string(),
            file_path: None,
            text: text.to_string(),
        }
    }

    #[test]
    fn a_saved_tab_named_by_its_file_and_an_unsaved_one_by_its_label() {
        assert_eq!(tab("Query 3", "select 1").display_name(), "Query 3");
        let saved = TabSnapshot {
            label: "Query 1".to_string(),
            file_path: Some(PathBuf::from("/tmp/reports/daily.sql")),
            text: "select 1".to_string(),
        };
        assert_eq!(saved.display_name(), "daily.sql");
    }

    #[test]
    fn an_oversized_tab_is_skipped_and_named_rather_than_truncated() {
        let big = "x".repeat(MAX_SNAPSHOT_TAB_BYTES + 1);
        let (snapshot, skipped) = SessionSnapshot::from_dirty_tabs(vec![
            tab("Query 1", "select 1"),
            tab("Query 2", &big),
        ]);
        assert_eq!(snapshot.tabs.len(), 1);
        assert_eq!(snapshot.tabs[0].label, "Query 1");
        assert_eq!(skipped, vec!["Query 2".to_string()]);
    }

    #[test]
    fn a_tab_exactly_at_the_limit_is_kept() {
        let exact = "x".repeat(MAX_SNAPSHOT_TAB_BYTES);
        let (snapshot, skipped) = SessionSnapshot::from_dirty_tabs(vec![tab("Query 1", &exact)]);
        assert_eq!(snapshot.tabs.len(), 1);
        assert!(skipped.is_empty());
    }

    #[test]
    fn the_change_key_moves_with_every_field_that_gets_persisted() {
        let base = SessionSnapshot {
            tabs: vec![tab("Query 1", "select 1")],
        };
        let mut renamed = base.clone();
        renamed.tabs[0].label = "Query 2".to_string();
        let mut retyped = base.clone();
        retyped.tabs[0].text = "select 2".to_string();
        let mut relocated = base.clone();
        relocated.tabs[0].file_path = Some(PathBuf::from("/tmp/a.sql"));
        let mut added = base.clone();
        added.tabs.push(tab("Query 2", "select 1"));

        let base_key = base.change_key();
        for other in [renamed, retyped, relocated, added] {
            assert_ne!(base_key, other.change_key());
        }
        assert_eq!(base_key, base.clone().change_key());
    }

    #[test]
    fn reordered_tabs_are_a_different_snapshot() {
        // Restoring tabs in the wrong order is a visible difference, so the
        // autosave tick must not treat a reorder as "nothing changed".
        let first = SessionSnapshot {
            tabs: vec![tab("Query 1", "a"), tab("Query 2", "b")],
        };
        let second = SessionSnapshot {
            tabs: vec![tab("Query 2", "b"), tab("Query 1", "a")],
        };
        assert_ne!(first.change_key(), second.change_key());
    }

    #[test]
    fn a_snapshot_survives_a_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!(
            "space-query-local-history-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let path = dir.join("unsaved_tabs.json");
        let snapshot = SessionSnapshot {
            tabs: vec![
                tab("Query 1", "select 1 from dual;\n-- 한글 주석\n"),
                TabSnapshot {
                    label: "Query 2".to_string(),
                    file_path: Some(PathBuf::from("/tmp/a.sql")),
                    text: String::new(),
                },
            ],
        };
        save_to_path(&snapshot, &path).expect("save");
        let restored: SessionSnapshot =
            serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("parse");
        assert_eq!(restored, snapshot);
        // The temporary file must be gone, not left beside the real one.
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .expect("dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "left behind {leftovers:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_snapshot_is_never_written() {
        let (snapshot, skipped) = SessionSnapshot::from_dirty_tabs(Vec::new());
        assert!(snapshot.is_empty());
        assert!(skipped.is_empty());
    }
}
