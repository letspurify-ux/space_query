use fltk::{
    app,
    draw::set_cursor,
    enums::{Cursor, Event, Key},
    prelude::*,
    text::{PositionType, TextBuffer, TextEditor},
};
use std::cell::Cell;
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
    detect_sql_context, sql_context_for_phase, ColumnMeta, ForeignKeyMeta, IntellisenseData,
    IntellisensePopup, QualifiedMemberKind, SignatureLabel, SignaturePopup, SqlContext,
    SuggestionDetail,
};
use crate::ui::intellisense_context;
use crate::ui::text_buffer_access;
use crate::ui::FindReplaceDialog;

use super::intellisense_state::IntellisenseCancellation;
use super::*;

const MAX_MERGED_SUGGESTIONS: usize = 100;
// One physical press may dispatch both KeyDown and Shortcut. A missing KeyUp
// must not keep suppressing later presses indefinitely.
const CTRL_ENTER_DUPLICATE_WINDOW: Duration = Duration::from_millis(100);
const COLUMN_LOAD_WORKER_COUNT: usize = 4;
const COLUMN_LOAD_CONTEXT_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(5),
    Duration::from_millis(10),
    Duration::from_millis(20),
];
const INTELLISENSE_PARSE_POLL_INTERVAL_SECONDS: f64 = 0.01;
const INTELLISENSE_DEFERRED_HIDE_RETRIES: u8 = 3;
const INTELLISENSE_POPUP_LOCK_RETRY_SECONDS: f64 = 0.01;
const INTELLISENSE_POPUP_LOCK_MAX_RETRIES: u8 = 20;
const SIGNATURE_POPUP_LOCK_RETRY_SECONDS: f64 = 0.01;
const SIGNATURE_POPUP_LOCK_MAX_RETRIES: u8 = 20;
const SIGNATURE_METADATA_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_INTELLISENSE_RECURSION_DEPTH: usize = 64;

thread_local! {
    static INTELLISENSE_RECURSION_DEPTH: Cell<usize> = const { Cell::new(0) };
}

struct IntellisenseRecursionGuard;

impl IntellisenseRecursionGuard {
    fn try_enter() -> Option<Self> {
        INTELLISENSE_RECURSION_DEPTH.with(|depth| {
            let current = depth.get();
            if current >= MAX_INTELLISENSE_RECURSION_DEPTH {
                None
            } else {
                depth.set(current + 1);
                Some(Self)
            }
        })
    }
}

impl Drop for IntellisenseRecursionGuard {
    fn drop(&mut self) {
        INTELLISENSE_RECURSION_DEPTH.with(|depth| {
            depth.set(depth.get().saturating_sub(1));
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavigationKeyupState {
    Idle,
    RestoreCursor { anchor: i32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EnterKeyupSuppression {
    None,
    PopupConfirm,
    CtrlEnterExecute(std::time::Instant),
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

type SharedTextSlice = ChunkedTextSlice;

#[derive(Clone)]
struct CursorAnalysisSnapshot {
    shared_sql_context: Option<SharedSqlContextSnapshot>,
    fast_text: SharedTextSlice,
    fast_start: usize,
    cursor_pos: usize,
    fast_initial_lex_mode: crate::sql_parser_engine::LexMode,
}

impl CursorAnalysisSnapshot {
    fn cursor_in_fast_text(&self) -> usize {
        self.cursor_pos
            .saturating_sub(self.fast_start)
            .min(self.fast_text.len())
    }

    fn fast_end(&self) -> usize {
        self.fast_start.saturating_add(self.fast_text.len())
    }

    fn shared_fast_slice(&self, start: usize, end: usize) -> SharedTextSlice {
        self.fast_text.subslice(start, end)
    }
}

struct CapturedCursorContext {
    analysis: Arc<CursorAnalysisSnapshot>,
    prefix: String,
    word_start: usize,
    qualifier: Option<String>,
    raw_qualifier: Option<String>,
    signature_scan_text: SharedTextSlice,
    signature_scan_initial_lex_mode: crate::sql_parser_engine::LexMode,
    text_after_cursor: SharedTextSlice,
}

enum CursorContextCapture {
    Suppressed,
    Ready(CapturedCursorContext),
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
    cursor_analysis: Arc<CursorAnalysisSnapshot>,
    signature_scan_text: SharedTextSlice,
    signature_scan_initial_lex_mode: crate::sql_parser_engine::LexMode,
    text_after_cursor: SharedTextSlice,
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
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    include!("tests.rs");
}
