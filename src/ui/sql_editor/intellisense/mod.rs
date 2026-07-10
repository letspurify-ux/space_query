use fltk::{
    app,
    draw::set_cursor,
    enums::{Cursor, Event, Key},
    prelude::*,
    text::{PositionType, TextBuffer, TextEditor},
};
use std::collections::{HashMap, HashSet};
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, OnceLock};
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

use oracle::Connection;

use crate::db::{ObjectBrowser, ProcedureArgument, SequenceInfo, SharedConnection};
use crate::sql_text;
use crate::ui::intellisense::{
    detect_sql_context, get_word_at_cursor, sql_context_for_phase, ColumnMeta, ForeignKeyMeta,
    IntellisenseData, IntellisensePopup, QualifiedMemberKind, SignatureLabel, SignaturePopup,
    SqlContext, SuggestionDetail,
};
use crate::ui::intellisense_context;
use crate::ui::text_buffer_access;
use crate::ui::FindReplaceDialog;

use super::*;

const MAX_MERGED_SUGGESTIONS: usize = 100;
const KEYUP_INTELLISENSE_DEBOUNCE_MS: u64 = 120;
const COLUMN_LOAD_WORKER_COUNT: usize = 4;
const INTELLISENSE_PARSE_POLL_INTERVAL_SECONDS: f64 = 0.01;
const INTELLISENSE_DEFERRED_HIDE_RETRIES: u8 = 3;
const SIGNATURE_POPUP_DEFERRED_HIDE_RETRIES: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavigationKeyupState {
    Idle,
    RestoreCursor { anchor: i32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EnterKeyupSuppression {
    None,
    PopupConfirm,
    CtrlEnterExecute,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    ImeCompositionEnter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingDndDrop {
    position: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DndDropState {
    Idle,
    AwaitingPaste(PendingDndDrop),
}

#[derive(Clone)]
struct IntellisenseTriggerSnapshot {
    request_generation: u64,
    buffer_revision: u64,
    cursor_pos: i32,
    cursor_pos_usize: usize,
    preferred_db_type: crate::db::connection::DatabaseType,
    prefix: String,
    word_start: usize,
    qualifier: Option<String>,
    raw_qualifier: Option<String>,
    signature_scan_text: String,
    text_after_cursor: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NormalizedIntellisenseContext {
    text: String,
    cursor_byte: usize,
}

#[derive(Clone)]
struct ColumnLoadTask {
    table_key: String,
    connection: SharedConnection,
    sender: mpsc::Sender<ColumnLoadUpdate>,
    /// When true the task loads the table's foreign keys instead of its
    /// columns. Foreign keys are fetched lazily (only for JOIN auto-join) so
    /// ordinary column completion stays a single query per table.
    foreign_keys: bool,
}

enum ColumnLoadWorkerMessage {
    Task(ColumnLoadTask),
    Shutdown,
}

struct ColumnLoadWorkerPool {
    worker_senders: Vec<mpsc::Sender<ColumnLoadWorkerMessage>>,
    worker_handles: Mutex<Vec<JoinHandle<()>>>,
    next_worker: AtomicUsize,
    shutdown: Arc<AtomicBool>,
}

impl ColumnLoadWorkerPool {
    fn enqueue(&self, task: ColumnLoadTask) -> Result<(), ColumnLoadTask> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err(task);
        }
        let worker_count = self.worker_senders.len();
        if worker_count == 0 {
            return Err(task);
        }

        let next = self.next_worker.fetch_add(1, Ordering::Relaxed);
        let Some(index) = next.checked_rem(worker_count) else {
            crate::utils::logging::log_error(
                "sql_editor::intellisense::column_loader",
                "failed to select column-load worker: worker count is zero",
            );
            return Err(task);
        };

        let task_for_err = task.clone();
        self.worker_senders[index]
            .send(ColumnLoadWorkerMessage::Task(task))
            .map_err(|_| task_for_err)
    }

    fn shutdown(&self) {
        if self.shutdown.swap(true, Ordering::AcqRel) {
            return;
        }
        for sender in &self.worker_senders {
            let _ = sender.send(ColumnLoadWorkerMessage::Shutdown);
        }

        let handles = {
            let mut guard = match self.worker_handles.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            std::mem::take(&mut *guard)
        };

        if handles.is_empty() {
            return;
        }

        let spawn_result = thread::Builder::new()
            .name("intellisense-column-worker-reaper".to_string())
            .spawn(move || {
                for handle in handles {
                    if let Err(err) = handle.join() {
                        crate::utils::logging::log_error(
                            "sql_editor::intellisense::column_loader",
                            &format!("column worker join failed: {:?}", err),
                        );
                    }
                }
            });
        if let Err(err) = spawn_result {
            crate::utils::logging::log_error(
                "sql_editor::intellisense::column_loader",
                &format!("failed to start column worker reaper: {err}"),
            );
        }
    }
}

static COLUMN_LOAD_WORKER_POOL: OnceLock<ColumnLoadWorkerPool> = OnceLock::new();

impl SqlEditorWidget {
    const INTELLISENSE_POPUP_WIDTH: i32 = 320;
    const INTELLISENSE_POPUP_Y_OFFSET: i32 = 20;
}

include!("helpers.rs");
include!("runtime.rs");
include!("local_symbols.rs");
include!("completion.rs");
include!("context.rs");
include!("popup.rs");

#[cfg(test)]
#[allow(dead_code)]
mod intellisense_regression_tests {
    use super::*;
    use crate::db::create_shared_connection;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    include!("tests.rs");
}
