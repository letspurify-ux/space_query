use oracle::sql_type::OracleType;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

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
    /// True when the value came from the bind-parameter prompt rather than a
    /// `VARIABLE` declaration.
    ///
    /// The two must stay distinguishable: a declaration is a standing statement
    /// about the bind and is never asked about again, while a prompted value is
    /// only the previous answer and is asked about on every run. Without this
    /// flag the first prompt would look exactly like a declaration to the next
    /// one, and the value would silently freeze.
    pub prompted: bool,
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

static NEXT_SPOOL_OWNER_ID: AtomicU64 = AtomicU64::new(1);
static SPOOL_PATH_OWNERS: OnceLock<Mutex<HashMap<PathBuf, u64>>> = OnceLock::new();

#[derive(Debug)]
pub(crate) struct SpoolOwner {
    id: u64,
}

impl SpoolOwner {
    fn new() -> Self {
        Self {
            id: NEXT_SPOOL_OWNER_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
}

impl Drop for SpoolOwner {
    fn drop(&mut self) {
        spool_path_owners().retain(|_, owner_id| *owner_id != self.id);
    }
}

fn spool_path_owners() -> std::sync::MutexGuard<'static, HashMap<PathBuf, u64>> {
    SPOOL_PATH_OWNERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug)]
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
    pub(crate) spool_owner: SpoolOwner,
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
            spool_owner: SpoolOwner::new(),
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
        Self {
            data_type,
            value,
            prompted: false,
        }
    }

    /// A bind carrying an answer from the bind-parameter prompt.
    ///
    /// A `REFCURSOR` answer names an OUT parameter rather than a value, so it
    /// starts empty exactly as a `VARIABLE rc REFCURSOR` declaration does.
    pub fn from_prompt(data_type: BindDataType, value: Option<String>) -> Self {
        let value = match data_type {
            BindDataType::RefCursor => BindValue::Cursor(None),
            _ => BindValue::Scalar(value),
        };
        Self {
            data_type,
            value,
            prompted: true,
        }
    }
}

impl SessionState {
    pub fn for_connection(db_type: DatabaseType) -> Self {
        let mut session = Self::default();
        session.set_connection_db_type(db_type);
        session
    }

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
        // current backend. Preserve the backend while restoring that backend's
        // defaults, without briefly exposing Oracle defaults for MySQL/MariaDB.
        let db_type = self.db_type;
        self.reset_for_connection(db_type);
    }

    pub fn reset_for_connection(&mut self, db_type: DatabaseType) {
        *self = Self::for_connection(db_type);
    }

    pub fn claim_spool_path(&mut self, path: PathBuf, truncate: bool) -> Result<(), String> {
        let mut owners = spool_path_owners();
        if let Some(owner_id) = owners.get(&path) {
            if *owner_id != self.spool_owner.id {
                return Err(format!(
                    "Spool file is already in use by another query tab: {}",
                    path.display()
                ));
            }
        }

        if let Some(previous_path) = self.spool_path.as_ref() {
            if previous_path != &path && owners.get(previous_path) == Some(&self.spool_owner.id) {
                owners.remove(previous_path);
            }
        }
        owners.insert(path.clone(), self.spool_owner.id);
        self.spool_path = Some(path);
        self.spool_truncate = truncate;
        Ok(())
    }

    pub fn clear_spool_path(&mut self) {
        if let Some(path) = self.spool_path.take() {
            let mut owners = spool_path_owners();
            if owners.get(&path) == Some(&self.spool_owner.id) {
                owners.remove(&path);
            }
        }
        self.spool_truncate = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_preserves_current_backend_and_restores_its_defaults() {
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
        assert!(!session.define_enabled);

        session.reset_for_connection(DatabaseType::Oracle);
        assert_eq!(
            session.db_type,
            DatabaseType::Oracle,
            "connection transitions must atomically reset and stamp the new backend"
        );
        assert!(session.define_enabled);
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

    #[test]
    fn spool_path_has_one_session_owner_and_off_releases_it() {
        let path = PathBuf::from("/tmp/space_query_spool_path_owner_test.log");
        let mut first = SessionState::default();
        let mut second = SessionState::default();

        first
            .claim_spool_path(path.clone(), true)
            .expect("first tab should own the spool path");
        assert!(second.claim_spool_path(path.clone(), false).is_err());

        first.clear_spool_path();
        second
            .claim_spool_path(path, false)
            .expect("SPOOL OFF should release ownership");
    }

    #[test]
    fn resetting_session_releases_spool_path() {
        let path = PathBuf::from("/tmp/space_query_spool_path_reset_test.log");
        let mut first = SessionState::default();
        first
            .claim_spool_path(path.clone(), true)
            .expect("first tab should own the spool path");

        first.reset_for_connection(DatabaseType::Oracle);

        let mut second = SessionState::default();
        second
            .claim_spool_path(path, false)
            .expect("connection transition should release spool ownership");
    }
}
