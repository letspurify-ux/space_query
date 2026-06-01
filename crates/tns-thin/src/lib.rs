pub mod connect;
pub mod exec;
mod oracle_zones;
pub mod pool;
pub mod session;

use std::fmt;
use std::sync::Mutex;

use once_cell::sync::OnceCell;

pub use connect::{ConnectOptions, ConnectTarget, OracleNetConnector};
pub use pool::OracleThinSessionPool;
pub use session::{OracleThinCancelHandle, OracleThinConfig, OracleThinSession};

type ConnectPhaseLogger = Box<dyn Fn(&str, &str) + Send + Sync + 'static>;

static CONNECT_PHASE_LOGGER: OnceCell<Mutex<Option<ConnectPhaseLogger>>> = OnceCell::new();

pub fn set_connect_phase_logger(logger: ConnectPhaseLogger) -> Result<(), OracleThinError> {
    let slot = CONNECT_PHASE_LOGGER.get_or_init(|| Mutex::new(None));
    let mut guard = slot
        .lock()
        .map_err(|_| OracleThinError::new("connect phase logger lock poisoned"))?;
    *guard = Some(logger);
    Ok(())
}

pub(crate) fn log_connect_phase(phase: &str, detail: &str) {
    let Some(slot) = CONNECT_PHASE_LOGGER.get() else {
        return;
    };
    let Ok(guard) = slot.lock() else {
        return;
    };
    if let Some(logger) = guard.as_ref() {
        logger(phase, detail);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OracleThinError {
    message: String,
    cursor_id: Option<u32>,
}

impl OracleThinError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cursor_id: None,
        }
    }

    pub(crate) fn with_cursor_id(message: impl Into<String>, cursor_id: Option<u32>) -> Self {
        Self {
            message: message.into(),
            cursor_id,
        }
    }

    pub(crate) fn cursor_id(&self) -> Option<u32> {
        self.cursor_id
    }
}

impl fmt::Display for OracleThinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OracleThinError {}

impl From<std::io::Error> for OracleThinError {
    fn from(value: std::io::Error) -> Self {
        Self::new(value.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OracleDateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub nanosecond: u32,
    pub timezone_offset_minutes: Option<i16>,
    pub timezone_region_id: Option<u16>,
}

impl OracleDateTime {
    pub fn timezone_suffix(&self) -> Option<String> {
        if let Some(offset) = self.timezone_offset_minutes {
            let sign = if offset < 0 { '-' } else { '+' };
            let abs = offset.unsigned_abs();
            return Some(format!("{sign}{:02}:{:02}", abs / 60, abs % 60));
        }
        let region_id = self.timezone_region_id?;
        Some(match oracle_zones::oracle_zone_name(region_id) {
            Some(name) => format!(" {name}"),
            None => format!(" TZR#{region_id}"),
        })
    }
}
