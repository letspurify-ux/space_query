use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

const APP_DIR_NAME: &str = "space_query";
const LOG_FILE_NAME: &str = "app.log.json";
const CRASH_LOG_FILE_NAME: &str = "crash.log";
const MAX_LOG_ENTRIES: usize = 100;
const LOG_WRITER_RESPONSE_TIMEOUT_DEFAULT_SECS: u64 = 15;

fn app_data_base_dir() -> Option<PathBuf> {
    if let Some(path) = dirs::data_dir() {
        return Some(path);
    }
    if let Some(home) = dirs::home_dir() {
        return Some(home.join(".local").join("share"));
    }
    None
}

fn log_writer_response_timeout() -> Duration {
    std::env::var("SPACE_QUERY_LOG_TIMEOUT_SECS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(LOG_WRITER_RESPONSE_TIMEOUT_DEFAULT_SECS))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

impl LogLevel {
    pub fn label(&self) -> &'static str {
        match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warning => "WARN",
            LogLevel::Error => "ERROR",
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: LogLevel,
    pub source: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppLog {
    pub entries: VecDeque<LogEntry>,
}

impl AppLog {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }

    fn log_path() -> Option<PathBuf> {
        app_data_base_dir().map(|mut p| {
            p.push(APP_DIR_NAME);
            p.push(LOG_FILE_NAME);
            p
        })
    }

    fn preserve_corrupt_log_file(path: &PathBuf) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        let backup_path = path.with_extension(format!("corrupt.{}.json", timestamp));
        match fs::rename(path, &backup_path) {
            Ok(()) => {
                eprintln!("Corrupt app log was moved to {}", backup_path.display());
            }
            Err(err) => {
                eprintln!(
                    "Failed to preserve corrupt app log file {}: {}",
                    path.display(),
                    err
                );
            }
        }
    }

    pub fn load() -> Self {
        if let Some(path) = Self::log_path() {
            if path.exists() {
                match fs::read_to_string(&path) {
                    Ok(content) => match serde_json::from_str::<Self>(&content) {
                        Ok(mut log) => {
                            log.entries.truncate(MAX_LOG_ENTRIES);
                            return log;
                        }
                        Err(err) => {
                            eprintln!("Failed to parse app log file {}: {}", path.display(), err);
                            Self::preserve_corrupt_log_file(&path);
                        }
                    },
                    Err(err) => {
                        eprintln!("Failed to read app log file {}: {}", path.display(), err);
                    }
                }
            }
        }
        Self::new()
    }

    fn save_to_path(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let tmp_path = path.with_file_name(format!(
            "{}.tmp.{}.{}",
            LOG_FILE_NAME,
            std::process::id(),
            timestamp
        ));

        let write_result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let mut file = fs::File::create(&tmp_path)?;
            serde_json::to_writer(&mut file, self)?;
            file.flush()?;
            file.sync_all()?;
            Ok(())
        })();

        if let Err(err) = write_result {
            let _ = fs::remove_file(&tmp_path);
            return Err(err);
        }

        if let Err(err) = fs::rename(&tmp_path, path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(Box::new(err));
        }

        Ok(())
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::log_path()
            .ok_or_else(|| std::io::Error::other("Log directory is unavailable"))?;
        self.save_to_path(&path)?;
        Ok(())
    }

    pub fn add_entry(&mut self, entry: LogEntry) {
        self.entries.push_front(entry);
        self.entries.truncate(MAX_LOG_ENTRIES);
    }
}

impl Default for AppLog {
    fn default() -> Self {
        Self::new()
    }
}

enum LogCommand {
    Write(LogEntry),
    Clear,
    Flush(mpsc::Sender<Result<(), String>>),
}

fn spawn_log_writer() -> mpsc::Sender<LogCommand> {
    let (sender, receiver) = mpsc::channel::<LogCommand>();
    thread::spawn(move || {
        let mut log = AppLog::load();
        loop {
            let cmd = match receiver.recv() {
                Ok(cmd) => cmd,
                Err(_) => break,
            };

            match cmd {
                LogCommand::Write(entry) => {
                    log.add_entry(entry);
                    if let Err(err) = log.save() {
                        eprintln!("Log save error: {err}");
                    }
                }
                LogCommand::Clear => {
                    log.entries.clear();
                    if let Err(err) = log.save() {
                        eprintln!("Log clear save error: {err}");
                    }
                }
                LogCommand::Flush(reply) => {
                    let _ = reply.send(Ok(()));
                }
            }
        }
    });
    sender
}

fn log_writer_handle() -> &'static Mutex<mpsc::Sender<LogCommand>> {
    static LOG_WRITER: OnceLock<Mutex<mpsc::Sender<LogCommand>>> = OnceLock::new();
    LOG_WRITER.get_or_init(|| Mutex::new(spawn_log_writer()))
}

fn log_writer_sender() -> mpsc::Sender<LogCommand> {
    log_writer_handle()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

fn send_log_command(command: LogCommand) -> Result<(), mpsc::SendError<LogCommand>> {
    let sender = log_writer_sender();
    let command = match sender.send(command) {
        Ok(()) => return Ok(()),
        Err(err) => err.0,
    };
    let mut guard = log_writer_handle()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = spawn_log_writer();
    guard.send(command)
}

pub fn flush_log_writer() -> Result<(), String> {
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    if send_log_command(LogCommand::Flush(tx)).is_err() {
        return Err("Log writer is not available".to_string());
    }

    let timeout = log_writer_response_timeout();
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let (retry_tx, retry_rx) = mpsc::channel::<Result<(), String>>();
            if send_log_command(LogCommand::Flush(retry_tx)).is_err() {
                return Err("Log writer is not available".to_string());
            }
            match retry_rx.recv_timeout(timeout) {
                Ok(result) => result,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    Err("Timed out while waiting for log persistence".to_string())
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    Err("Log writer disconnected while flushing".to_string())
                }
            }
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("Log writer disconnected while flushing".to_string())
        }
    }
}

/// In-memory ring buffer so the UI can show recent entries without
/// re-reading the file every time the dialog is opened.
fn in_memory_log() -> &'static Mutex<VecDeque<LogEntry>> {
    static BUFFER: OnceLock<Mutex<VecDeque<LogEntry>>> = OnceLock::new();
    BUFFER.get_or_init(|| {
        let log = AppLog::load();
        Mutex::new(log.entries)
    })
}

pub fn log(level: LogLevel, source: &str, message: &str) {
    let entry = LogEntry {
        timestamp: Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        level,
        source: source.to_string(),
        message: message.to_string(),
    };

    // Update in-memory buffer
    if let Ok(mut buf) = in_memory_log().lock() {
        buf.push_front(entry.clone());
        buf.truncate(MAX_LOG_ENTRIES);
    }

    // Persist via background writer
    if let Err(send_err) = send_log_command(LogCommand::Write(entry)) {
        if let LogCommand::Write(failed_entry) = send_err.0 {
            let mut log = AppLog::load();
            log.add_entry(failed_entry);
            if let Err(err) = log.save() {
                eprintln!("Failed to persist log entry through fallback path: {err}");
            }
        }
    }
}

pub fn log_info(source: &str, message: &str) {
    log(LogLevel::Info, source, message);
}

pub fn log_warning(source: &str, message: &str) {
    log(LogLevel::Warning, source, message);
}

pub fn log_error(source: &str, message: &str) {
    log(LogLevel::Error, source, message);
}

#[allow(dead_code)]
pub fn log_debug(source: &str, message: &str) {
    log(LogLevel::Debug, source, message);
}

/// Return a snapshot of the in-memory log entries.
pub fn get_log_entries() -> Vec<LogEntry> {
    in_memory_log()
        .lock()
        .map(|buf| buf.iter().cloned().collect())
        .unwrap_or_default()
}

/// Clear all log entries (in-memory + persisted).
pub fn clear_log() -> Result<(), String> {
    match send_log_command(LogCommand::Clear) {
        Ok(()) => {
            flush_log_writer()?;
            if let Ok(mut buf) = in_memory_log().lock() {
                buf.clear();
            }
            Ok(())
        }
        Err(send_err) => {
            if let LogCommand::Clear = send_err.0 {
                let mut log = AppLog::load();
                log.entries.clear();
                log.save()
                    .map_err(|err| format!("Failed to clear persisted log: {err}"))?;
                if let Ok(mut buf) = in_memory_log().lock() {
                    buf.clear();
                }
                Ok(())
            } else {
                Err("Failed to clear application log".to_string())
            }
        }
    }
}

// ── Crash log helpers ──

pub fn crash_log_path() -> Option<PathBuf> {
    app_data_base_dir().map(|mut p| {
        p.push(APP_DIR_NAME);
        p.push(CRASH_LOG_FILE_NAME);
        p
    })
}

/// Write a crash report synchronously (called from panic hook).
pub fn write_crash_log(info: &str) {
    if let Some(path) = crash_log_path() {
        if let Some(parent) = path.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                eprintln!(
                    "Failed to create crash log directory {}: {}",
                    parent.display(),
                    err
                );
                return;
            }
        }
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
        let report = format!(
            "=== SPACE Query Crash Report ===\nTimestamp: {}\n\n{}\n\n",
            timestamp, info
        );
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(mut file) => {
                if let Err(err) = std::io::Write::write_all(&mut file, report.as_bytes()) {
                    eprintln!("Failed to write crash log {}: {}", path.display(), err);
                }
            }
            Err(err) => {
                eprintln!("Failed to open crash log {}: {}", path.display(), err);
            }
        }
    }
}

/// Read and remove the crash log if it exists. Returns the content.
pub fn take_crash_log() -> Option<String> {
    let path = crash_log_path()?;
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    if let Err(err) = fs::remove_file(&path) {
        eprintln!(
            "Failed to remove crash log after reading {}: {}",
            path.display(),
            err
        );
    }
    Some(content)
}

#[cfg(test)]
mod logging_tests {
    use super::*;

    #[test]
    fn log_level_label_returns_expected_strings() {
        assert_eq!(LogLevel::Debug.label(), "DEBUG");
        assert_eq!(LogLevel::Info.label(), "INFO");
        assert_eq!(LogLevel::Warning.label(), "WARN");
        assert_eq!(LogLevel::Error.label(), "ERROR");
    }

    #[test]
    fn app_log_add_entry_inserts_at_front_and_truncates() {
        let mut log = AppLog::new();
        for i in 0..10 {
            log.add_entry(LogEntry {
                timestamp: format!("t{i}"),
                level: LogLevel::Info,
                source: "test".to_string(),
                message: format!("msg{i}"),
            });
        }
        assert_eq!(log.entries.len(), 10);
        assert_eq!(log.entries[0].message, "msg9");
        assert_eq!(log.entries[9].message, "msg0");
    }

    #[test]
    fn app_log_save_writes_complete_json_without_tmp_file_leftover() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "space_query_log_save_test_{}_{}",
            std::process::id(),
            unique
        ));
        let path = dir.join(LOG_FILE_NAME);
        let mut log = AppLog::new();
        log.add_entry(LogEntry {
            timestamp: "t".to_string(),
            level: LogLevel::Info,
            source: "test".to_string(),
            message: "saved".to_string(),
        });

        log.save_to_path(&path).expect("log save should succeed");

        let saved = fs::read_to_string(&path).expect("saved log should be readable");
        let parsed: AppLog = serde_json::from_str(&saved).expect("saved log should be valid JSON");
        assert_eq!(parsed.entries[0].message, "saved");
        let leftovers = fs::read_dir(&dir)
            .expect("log test dir should be readable")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp."))
            .count();
        assert_eq!(leftovers, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_level_display_matches_label() {
        assert_eq!(format!("{}", LogLevel::Warning), "WARN");
    }
}
