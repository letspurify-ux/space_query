#![allow(clippy::cargo, clippy::pedantic)]

pub mod connect;
pub mod exec;
mod oracle_zones;
pub mod pool;
pub mod session;

use std::fmt;
use std::sync::Mutex;

use once_cell::sync::OnceCell;

pub use connect::{ConnectOptions, ConnectTarget, OracleNetConnector, OracleNetServerType};
pub use pool::OracleThinSessionPool;
pub use session::{
    OracleThinAppContext, OracleThinAuthMode, OracleThinCancelHandle, OracleThinConfig,
    OracleThinEndUserSecurityContext, OracleThinPurity, OracleThinSession, OracleThinWarning,
    ORACLE_THIN_CALL_TIMEOUT_MESSAGE,
};

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
    code: Option<u32>,
    full_code: Option<String>,
    offset: Option<i32>,
    recoverable: bool,
}

impl OracleThinError {
    pub fn new(message: impl Into<String>) -> Self {
        let message = message.into();
        let full_code = error_full_code(&message);
        Self {
            code: numeric_error_code(full_code.as_deref()),
            full_code,
            recoverable: message_is_recoverable_oracle_error(&message),
            message,
            cursor_id: None,
            offset: None,
        }
    }

    pub(crate) fn with_oracle_details(
        message: impl Into<String>,
        cursor_id: Option<u32>,
        code: u32,
        offset: i32,
    ) -> Self {
        Self {
            message: message.into(),
            cursor_id,
            code: Some(code),
            full_code: Some(format!("ORA-{code:05}")),
            offset: Some(offset),
            recoverable: oracle_code_is_recoverable(code),
        }
    }

    pub(crate) fn from_io_error(message: String) -> Self {
        Self {
            message,
            cursor_id: None,
            code: None,
            full_code: None,
            offset: None,
            recoverable: true,
        }
    }

    pub fn code(&self) -> Option<u32> {
        self.code
    }

    pub fn offset(&self) -> Option<i32> {
        self.offset
    }

    pub fn full_code(&self) -> Option<String> {
        self.full_code.clone()
    }

    pub fn is_recoverable(&self) -> bool {
        self.recoverable
    }

    pub(crate) fn cursor_id(&self) -> Option<u32> {
        self.cursor_id
    }

    pub(crate) fn with_offset(mut self, offset: i32) -> Self {
        self.offset = Some(offset);
        self
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
        Self::from_io_error(value.to_string())
    }
}

fn oracle_error_code(message: &str) -> Option<u32> {
    let full_code = message
        .find("ORA-")
        .and_then(|start| error_full_code(&message[start..]))?;
    numeric_error_code(Some(&full_code))
}

fn error_full_code(message: &str) -> Option<String> {
    for prefix in ["ORA-", "DPY-", "DPI-"] {
        let Some(start) = message.find(prefix).map(|start| start + prefix.len()) else {
            continue;
        };
        let digits = message[start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        if !digits.is_empty() {
            return Some(format!("{prefix}{digits}"));
        }
    }
    None
}

fn numeric_error_code(full_code: Option<&str>) -> Option<u32> {
    full_code?.get(4..)?.parse().ok()
}

fn message_is_recoverable_oracle_error(message: &str) -> bool {
    oracle_error_code(message).is_some_and(oracle_code_is_recoverable)
}

fn oracle_code_is_recoverable(code: u32) -> bool {
    matches!(
        code,
        28 | 1012 | 1033 | 1034 | 1089 | 1090 | 2396 | 3113 | 3114 | 3135
    ) || (12_500..=12_599).contains(&code)
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

#[cfg(test)]
mod tests {
    use super::OracleThinError;

    #[test]
    fn oracle_thin_error_reports_ora_and_dpy_full_codes() {
        let oracle = OracleThinError::new("ORA-01476: divisor is equal to zero");
        assert_eq!(oracle.code(), Some(1476));
        assert_eq!(oracle.full_code().as_deref(), Some("ORA-01476"));

        let driver = OracleThinError::new("DPY-2041: invalid SQL quoting");
        assert_eq!(driver.code(), Some(2041));
        assert_eq!(driver.full_code().as_deref(), Some("DPY-2041"));
    }
}
