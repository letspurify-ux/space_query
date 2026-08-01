use oracle::sql_type::OracleType;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::db::connection::DatabaseType;

#[derive(Debug, Clone)]
pub enum BindDataType {
    Number,
    Varchar2(u32),
    Date,
    Timestamp(u8),
    RefCursor,
    Clob,
}

#[derive(Debug, Clone)]
pub enum BindValue {
    Scalar(Option<String>),
    Cursor(Option<CursorResult>),
}

#[derive(Debug, Clone)]
pub struct BindVar {
    pub data_type: BindDataType,
    pub value: BindValue,
}

#[derive(Debug, Clone)]
pub struct CursorResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct ServerOutputConfig {
    pub enabled: bool,
    pub size: u32,
}

#[derive(Debug, Clone)]
pub struct CompiledObject {
    pub owner: Option<String>,
    pub object_type: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeMode {
    Sum,
    Count,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputeConfig {
    pub mode: ComputeMode,
    /// Label printed instead of `SUM`/`COUNT` when the command carried one.
    pub label: Option<String>,
    pub of_column: Option<String>,
    pub on_column: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SessionState {
    pub db_type: DatabaseType,
    pub binds: HashMap<String, BindVar>,
    pub displayed_cursor_binds: HashSet<String>,
    pub define_vars: HashMap<String, String>,
    pub column_new_values: HashMap<String, String>,
    pub server_output: ServerOutputConfig,
    pub last_compiled: Option<CompiledObject>,
    pub continue_on_error: bool,
    pub define_enabled: bool,
    pub define_char: char,
    pub concat_enabled: bool,
    pub concat_char: char,
    pub escape_enabled: bool,
    pub escape_char: char,
    pub scan_enabled: bool,
    pub verify_enabled: bool,
    pub echo_enabled: bool,
    pub timing_enabled: bool,
    pub feedback_enabled: bool,
    pub heading_enabled: bool,
    pub pagesize: u32,
    pub linesize: u32,
    pub trimspool_enabled: bool,
    pub trimout_enabled: bool,
    pub sqlblanklines_enabled: bool,
    pub tab_enabled: bool,
    pub colsep: String,
    pub null_text: String,
    pub break_column: Option<String>,
    pub compute: Option<ComputeConfig>,
    pub spool_path: Option<PathBuf>,
    pub spool_truncate: bool,
    /// MySQL DELIMITER state — custom statement terminator for stored routines.
    pub mysql_delimiter: Option<String>,
}

impl Default for ServerOutputConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            size: 1_000_000,
        }
    }
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            db_type: DatabaseType::default(),
            binds: HashMap::new(),
            displayed_cursor_binds: HashSet::new(),
            define_vars: HashMap::new(),
            column_new_values: HashMap::new(),
            server_output: ServerOutputConfig::default(),
            last_compiled: None,
            continue_on_error: false,
            define_enabled: true,
            define_char: '&',
            concat_enabled: true,
            concat_char: '.',
            escape_enabled: false,
            escape_char: '\\',
            scan_enabled: true,
            verify_enabled: false,
            echo_enabled: false,
            timing_enabled: false,
            feedback_enabled: true,
            heading_enabled: true,
            pagesize: 14,
            linesize: 80,
            trimspool_enabled: false,
            trimout_enabled: false,
            sqlblanklines_enabled: false,
            tab_enabled: true,
            colsep: " | ".to_string(),
            null_text: "NULL".to_string(),
            break_column: None,
            compute: None,
            spool_path: None,
            spool_truncate: false,
            mysql_delimiter: None,
        }
    }
}

impl BindDataType {
    pub fn oracle_type(&self) -> OracleType {
        match self {
            BindDataType::Number => OracleType::Number(0, 0),
            BindDataType::Varchar2(size) => OracleType::Varchar2(*size),
            BindDataType::Date => OracleType::Date,
            BindDataType::Timestamp(precision) => OracleType::Timestamp(*precision),
            BindDataType::RefCursor => OracleType::RefCursor,
            BindDataType::Clob => OracleType::CLOB,
        }
    }

    pub fn display(&self) -> String {
        match self {
            BindDataType::Number => "NUMBER".to_string(),
            BindDataType::Varchar2(size) => format!("VARCHAR2({})", size),
            BindDataType::Date => "DATE".to_string(),
            BindDataType::Timestamp(precision) => format!("TIMESTAMP({})", precision),
            BindDataType::RefCursor => "REFCURSOR".to_string(),
            BindDataType::Clob => "CLOB".to_string(),
        }
    }
}

impl BindVar {
    pub fn new(data_type: BindDataType) -> Self {
        let value = match data_type {
            BindDataType::RefCursor => BindValue::Cursor(None),
            _ => BindValue::Scalar(None),
        };
        Self { data_type, value }
    }
}

impl SessionState {
    pub fn normalize_name(name: &str) -> String {
        name.trim().trim_start_matches(':').to_uppercase()
    }

    pub fn set_connection_db_type(&mut self, db_type: DatabaseType) {
        if self.db_type != db_type {
            self.define_enabled = match db_type {
                DatabaseType::Oracle => true,
                DatabaseType::MySQL => false,
                DatabaseType::MariaDB => false,
            };
        }
        self.db_type = db_type;
    }

    pub fn reset(&mut self) {
        // Reset is used for client-side SQL*Plus/session settings within the
        // current backend. Connection transitions that change backend type set
        // db_type explicitly after reset/connect, so preserving it here avoids
        // briefly falling back to Oracle parsing on same-connection resets.
        let db_type = self.db_type;
        *self = Self::default();
        self.db_type = db_type;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_preserves_current_backend_until_connection_transition_restamps_it() {
        let mut session = SessionState {
            db_type: DatabaseType::MySQL,
            continue_on_error: true,
            mysql_delimiter: Some("//".to_string()),
            ..SessionState::default()
        };

        session.reset();

        assert_eq!(session.db_type, DatabaseType::MySQL);
        assert!(!session.continue_on_error);
        assert_eq!(session.mysql_delimiter, None);

        session.db_type = DatabaseType::Oracle;
        assert_eq!(
            session.db_type,
            DatabaseType::Oracle,
            "connection transition code must explicitly stamp the new backend after reset/connect"
        );
    }

    #[test]
    fn connection_transition_uses_define_substitution_only_by_default_for_oracle() {
        let mut session = SessionState::default();

        session.set_connection_db_type(DatabaseType::MySQL);
        assert!(!session.define_enabled);

        session.define_enabled = true;
        session.set_connection_db_type(DatabaseType::MySQL);
        assert!(
            session.define_enabled,
            "an explicit same-backend SET DEFINE choice must survive reconnect"
        );

        session.set_connection_db_type(DatabaseType::MariaDB);
        assert!(!session.define_enabled);

        session.set_connection_db_type(DatabaseType::Oracle);
        assert!(session.define_enabled);
    }
}
