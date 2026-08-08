use mysql::prelude::*;
use mysql::{Conn, Error as MysqlError, Row, Value as MysqlValue};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::db::connection::ConnectionInfo;
use crate::db::DatabaseType;
use crate::sql_text;

use super::executor::{
    ConstraintInfo, ForeignKeyInfo, IndexInfo, QueryExecutor, TableColumnDetail,
};
use super::types::{
    result_messages, ColumnInfo, ProcedureArgument, QueryCell, QueryResult, SqlValueKind,
};

pub struct MysqlExecutor;

pub struct MysqlObjectBrowser;

const MYSQL_CANCEL_IO_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub(crate) struct MysqlSessionTimeoutApplyError {
    apply_error: Box<MysqlError>,
    restore_error: Option<Box<MysqlError>>,
}

impl MysqlSessionTimeoutApplyError {
    fn new(apply_error: MysqlError, restore_error: Option<MysqlError>) -> Self {
        Self {
            apply_error: Box::new(apply_error),
            restore_error: restore_error.map(Box::new),
        }
    }

    pub(crate) fn apply_error(&self) -> &MysqlError {
        self.apply_error.as_ref()
    }

    pub(crate) fn restore_error(&self) -> Option<&MysqlError> {
        self.restore_error.as_deref()
    }

    pub(crate) fn restore_failed(&self) -> bool {
        self.restore_error.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MysqlSessionTimeoutRestore {
    max_execution_time: Option<u64>,
    max_statement_time: Option<String>,
    lock_wait_timeout: Option<u64>,
    innodb_lock_wait_timeout: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MysqlQueryTimeoutVariable {
    MaxExecutionTime,
    MaxStatementTime,
}

impl MysqlSessionTimeoutRestore {
    fn max_execution_time_statement(&self) -> Option<String> {
        self.max_execution_time
            .map(|value| format!("SET SESSION MAX_EXECUTION_TIME = {value}"))
    }

    fn max_statement_time_statement(&self) -> Option<String> {
        self.max_statement_time
            .as_deref()
            .map(|value| format!("SET SESSION max_statement_time = {value}"))
    }

    fn lock_wait_timeout_statement(&self) -> Option<String> {
        self.lock_wait_timeout
            .map(|value| format!("SET SESSION lock_wait_timeout = {value}"))
    }

    fn innodb_lock_wait_timeout_statement(&self) -> Option<String> {
        self.innodb_lock_wait_timeout
            .map(|value| format!("SET SESSION innodb_lock_wait_timeout = {value}"))
    }

    fn query_timeout_statement_for(&self, variable: MysqlQueryTimeoutVariable) -> Option<String> {
        match variable {
            MysqlQueryTimeoutVariable::MaxExecutionTime => self.max_execution_time_statement(),
            MysqlQueryTimeoutVariable::MaxStatementTime => self.max_statement_time_statement(),
        }
    }

    fn timeout_variable_should_be_applied(&self, variable_name: &str) -> bool {
        match variable_name {
            "lock_wait_timeout" => self.lock_wait_timeout.is_some(),
            "innodb_lock_wait_timeout" => self.innodb_lock_wait_timeout.is_some(),
            _ => true,
        }
    }

    pub(crate) fn restore_for_db<C: Queryable>(
        &self,
        conn: &mut C,
        db_type: DatabaseType,
    ) -> Result<(), MysqlError> {
        let provider = mysql_timeout_provider_for(db_type)?;
        // Restore the exact query-timeout session variable we captured. Calling
        // the generic "timeout = None" reset here would silently erase a user's
        // pre-existing MAX_EXECUTION_TIME / max_statement_time setting.
        let query_timeout_statement = self
            .query_timeout_statement_for(provider.primary_query_timeout_variable())
            .or_else(|| {
                provider
                    .fallback_query_timeout_variable()
                    .and_then(|variable| self.query_timeout_statement_for(variable))
            });
        // Any error from this sequence is a session-safety signal. Earlier
        // variables may already have been restored while later variables remain
        // dirty, so callers must discard the physical session on failure.
        if let Some(statement) = query_timeout_statement {
            conn.query_drop(statement.as_str())?;
        }
        if let Some(statement) = self.lock_wait_timeout_statement() {
            conn.query_drop(statement.as_str())?;
        }
        if let Some(statement) = self.innodb_lock_wait_timeout_statement() {
            conn.query_drop(statement.as_str())?;
        }
        Ok(())
    }
}

trait MysqlTimeoutProvider: Sync {
    fn primary_query_timeout_variable(&self) -> MysqlQueryTimeoutVariable;
    fn fallback_query_timeout_variable(&self) -> Option<MysqlQueryTimeoutVariable> {
        None
    }
    fn query_timeout_statement(&self, timeout: Option<Duration>) -> String;
    fn fallback_query_timeout_statement(&self, _timeout: Option<Duration>) -> Option<String> {
        None
    }
}

struct MySqlTimeoutProvider;
struct MariaDbTimeoutProvider;

static MYSQL_TIMEOUT_PROVIDER: MySqlTimeoutProvider = MySqlTimeoutProvider;
static MARIADB_TIMEOUT_PROVIDER: MariaDbTimeoutProvider = MariaDbTimeoutProvider;

fn mysql_timeout_provider_for(
    db_type: DatabaseType,
) -> Result<&'static dyn MysqlTimeoutProvider, MysqlError> {
    match db_type {
        DatabaseType::Oracle => Err(MysqlError::DriverError(mysql::DriverError::SetupError)),
        DatabaseType::MySQL => Ok(&MYSQL_TIMEOUT_PROVIDER),
        DatabaseType::MariaDB => Ok(&MARIADB_TIMEOUT_PROVIDER),
    }
}

impl MysqlTimeoutProvider for MySqlTimeoutProvider {
    fn primary_query_timeout_variable(&self) -> MysqlQueryTimeoutVariable {
        MysqlQueryTimeoutVariable::MaxExecutionTime
    }

    fn fallback_query_timeout_variable(&self) -> Option<MysqlQueryTimeoutVariable> {
        Some(MysqlQueryTimeoutVariable::MaxStatementTime)
    }

    fn query_timeout_statement(&self, timeout: Option<Duration>) -> String {
        MysqlExecutor::mysql_timeout_statement(timeout)
    }

    fn fallback_query_timeout_statement(&self, timeout: Option<Duration>) -> Option<String> {
        Some(MysqlExecutor::mariadb_timeout_statement(timeout))
    }
}

impl MysqlTimeoutProvider for MariaDbTimeoutProvider {
    fn primary_query_timeout_variable(&self) -> MysqlQueryTimeoutVariable {
        MysqlQueryTimeoutVariable::MaxStatementTime
    }

    fn query_timeout_statement(&self, timeout: Option<Duration>) -> String {
        MysqlExecutor::mariadb_timeout_statement(timeout)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MysqlStatementKind {
    Select,
    Dml,
    Commit,
    Rollback,
    Use,
    Call,
    Ddl,
}

#[derive(Debug, Clone)]
struct MysqlResultSetSnapshot {
    columns: Vec<ColumnInfo>,
    rows: Vec<Vec<String>>,
    affected_rows: u64,
    info: String,
}

impl MysqlExecutor {
    fn timeout_millis(timeout: Option<Duration>) -> u128 {
        timeout.map(|value| value.as_millis()).unwrap_or(0)
    }

    fn lock_wait_timeout_seconds(timeout: Duration) -> String {
        timeout.as_secs().max(1).to_string()
    }

    fn mysql_timeout_statement(timeout: Option<Duration>) -> String {
        format!(
            "SET SESSION MAX_EXECUTION_TIME = {}",
            Self::timeout_millis(timeout)
        )
    }

    fn mariadb_timeout_statement(timeout: Option<Duration>) -> String {
        let timeout_seconds = timeout.map(|value| value.as_secs_f64()).unwrap_or(0.0);
        format!("SET SESSION max_statement_time = {:.3}", timeout_seconds)
    }

    pub(crate) fn statement_sql_preserving_found_rows_for_db_type(
        db_type: DatabaseType,
        sql: &str,
        timeout: Option<Duration>,
        sets_found_rows: bool,
    ) -> String {
        match db_type {
            DatabaseType::Oracle => sql.to_string(),
            DatabaseType::MySQL => sql.to_string(),
            DatabaseType::MariaDB => {
                if let (true, Some(timeout)) = (sets_found_rows, timeout) {
                    return format!(
                        "SET STATEMENT max_statement_time = {:.3} FOR {sql}",
                        timeout.as_secs_f64()
                    );
                }
                sql.to_string()
            }
        }
    }

    fn lock_wait_timeout_statement(timeout: Duration) -> String {
        format!(
            "SET SESSION lock_wait_timeout = {}",
            Self::lock_wait_timeout_seconds(timeout)
        )
    }

    fn innodb_lock_wait_timeout_statement(timeout: Duration) -> String {
        format!(
            "SET SESSION innodb_lock_wait_timeout = {}",
            Self::lock_wait_timeout_seconds(timeout)
        )
    }

    fn read_session_u64_variable<C: Queryable>(
        conn: &mut C,
        variable_name: &str,
    ) -> Result<u64, MysqlError> {
        conn.query_first::<u64, _>(format!("SELECT @@SESSION.{variable_name}"))?
            .ok_or(MysqlError::DriverError(mysql::DriverError::SetupError))
    }

    fn read_session_string_variable<C: Queryable>(
        conn: &mut C,
        variable_name: &str,
    ) -> Result<String, MysqlError> {
        conn.query_first::<String, _>(format!("SELECT CAST(@@SESSION.{variable_name} AS CHAR)"))?
            .ok_or(MysqlError::DriverError(mysql::DriverError::SetupError))
    }

    fn optional_timeout_restore_value(
        variable_name: &str,
        read_result: Result<u64, MysqlError>,
    ) -> Result<Option<u64>, MysqlError> {
        match read_result {
            Ok(value) => Ok(Some(value)),
            Err(err) if Self::is_unknown_system_variable_error(&err, variable_name) => {
                crate::utils::logging::log_warning(
                    "mysql timeout restore",
                    &format!(
                        "Skipping unsupported @@SESSION.{variable_name} while capturing timeout restore state: {err}"
                    ),
                );
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    fn optional_timeout_restore_string_value(
        variable_name: &str,
        read_result: Result<String, MysqlError>,
    ) -> Result<Option<String>, MysqlError> {
        match read_result {
            Ok(value) => Ok(Some(value)),
            Err(err) if Self::is_unknown_system_variable_error(&err, variable_name) => {
                crate::utils::logging::log_warning(
                    "mysql timeout restore",
                    &format!(
                        "Skipping unsupported @@SESSION.{variable_name} while capturing timeout restore state: {err}"
                    ),
                );
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    fn capture_session_query_timeout_restore<C: Queryable>(
        conn: &mut C,
        variable: MysqlQueryTimeoutVariable,
        restore: &mut MysqlSessionTimeoutRestore,
    ) -> Result<bool, MysqlError> {
        match variable {
            MysqlQueryTimeoutVariable::MaxExecutionTime => {
                let value = Self::optional_timeout_restore_value(
                    "MAX_EXECUTION_TIME",
                    Self::read_session_u64_variable(conn, "MAX_EXECUTION_TIME"),
                )?;
                let captured = value.is_some();
                restore.max_execution_time = value;
                Ok(captured)
            }
            MysqlQueryTimeoutVariable::MaxStatementTime => {
                let value = Self::optional_timeout_restore_string_value(
                    "max_statement_time",
                    Self::read_session_string_variable(conn, "max_statement_time"),
                )?;
                let captured = value.is_some();
                restore.max_statement_time = value;
                Ok(captured)
            }
        }
    }

    fn capture_session_timeout_restore<C: Queryable>(
        conn: &mut C,
        db_type: DatabaseType,
    ) -> Result<MysqlSessionTimeoutRestore, MysqlError> {
        let provider = mysql_timeout_provider_for(db_type)?;
        let mut restore = MysqlSessionTimeoutRestore {
            max_execution_time: None,
            max_statement_time: None,
            lock_wait_timeout: None,
            innodb_lock_wait_timeout: None,
        };

        let primary_captured = Self::capture_session_query_timeout_restore(
            conn,
            provider.primary_query_timeout_variable(),
            &mut restore,
        )?;
        if !primary_captured {
            if let Some(variable) = provider.fallback_query_timeout_variable() {
                Self::capture_session_query_timeout_restore(conn, variable, &mut restore)?;
            }
        }

        restore.lock_wait_timeout = Self::optional_timeout_restore_value(
            "lock_wait_timeout",
            Self::read_session_u64_variable(conn, "lock_wait_timeout"),
        )?;
        restore.innodb_lock_wait_timeout = Self::optional_timeout_restore_value(
            "innodb_lock_wait_timeout",
            Self::read_session_u64_variable(conn, "innodb_lock_wait_timeout"),
        )?;
        Ok(restore)
    }

    fn is_unknown_system_variable_error(err: &MysqlError, variable_name: &str) -> bool {
        match err {
            MysqlError::MySqlError(server_err) => {
                server_err.code == 1193
                    || server_err
                        .message
                        .contains(&format!("Unknown system variable '{variable_name}'"))
            }
            _ => false,
        }
    }

    fn apply_session_timeout_for_db_with_restore_hint<C: Queryable>(
        conn: &mut C,
        timeout: Option<Duration>,
        db_type: DatabaseType,
        restore: Option<&MysqlSessionTimeoutRestore>,
    ) -> Result<(), MysqlError> {
        let provider = mysql_timeout_provider_for(db_type)?;
        let statement = provider.query_timeout_statement(timeout);
        match conn.query_drop(statement.as_str()) {
            Ok(()) => Ok(()),
            Err(err)
                if Self::is_unknown_system_variable_error(&err, "MAX_EXECUTION_TIME")
                    && provider.fallback_query_timeout_statement(timeout).is_some() =>
            {
                if let Some(fallback) = provider.fallback_query_timeout_statement(timeout) {
                    conn.query_drop(fallback.as_str())
                } else {
                    Err(err)
                }
            }
            Err(err) => Err(err),
        }?;
        if let Some(timeout) = timeout {
            if restore
                .map(|restore| restore.timeout_variable_should_be_applied("lock_wait_timeout"))
                .unwrap_or(true)
            {
                conn.query_drop(Self::lock_wait_timeout_statement(timeout).as_str())?;
            }
            if restore
                .map(|restore| {
                    restore.timeout_variable_should_be_applied("innodb_lock_wait_timeout")
                })
                .unwrap_or(true)
            {
                conn.query_drop(Self::innodb_lock_wait_timeout_statement(timeout).as_str())?;
            }
        }
        Ok(())
    }

    pub(crate) fn apply_session_timeout_with_restore_for_db<C: Queryable>(
        conn: &mut C,
        timeout: Option<Duration>,
        db_type: DatabaseType,
    ) -> Result<Option<MysqlSessionTimeoutRestore>, MysqlSessionTimeoutApplyError> {
        let Some(timeout) = timeout else {
            return Ok(None);
        };
        let restore = Some(
            Self::capture_session_timeout_restore(conn, db_type)
                .map_err(|err| MysqlSessionTimeoutApplyError::new(err, None))?,
        );
        Self::apply_session_timeout_with_captured_restore_for_db(
            conn,
            Some(timeout),
            db_type,
            restore,
        )
    }

    fn apply_session_timeout_with_captured_restore_for_db<C: Queryable>(
        conn: &mut C,
        timeout: Option<Duration>,
        db_type: DatabaseType,
        restore: Option<MysqlSessionTimeoutRestore>,
    ) -> Result<Option<MysqlSessionTimeoutRestore>, MysqlSessionTimeoutApplyError> {
        match Self::apply_session_timeout_for_db_with_restore_hint(
            conn,
            timeout,
            db_type,
            restore.as_ref(),
        ) {
            Ok(()) => Ok(restore),
            Err(err) => {
                let restore_error =
                    restore.and_then(|restore| restore.restore_for_db(conn, db_type).err());
                Err(MysqlSessionTimeoutApplyError::new(err, restore_error))
            }
        }
    }

    #[cfg(test)]
    fn classify_statement(sql: &str) -> MysqlStatementKind {
        Self::classify_statement_for_db_type(DatabaseType::MySQL, sql)
    }

    fn classify_statement_for_db_type(db_type: DatabaseType, sql: &str) -> MysqlStatementKind {
        match crate::db::query::statement_execution_profile_for_db_type(db_type, sql).result_kind {
            crate::db::query::StatementResultKind::Select => MysqlStatementKind::Select,
            crate::db::query::StatementResultKind::Dml => MysqlStatementKind::Dml,
            crate::db::query::StatementResultKind::Commit => MysqlStatementKind::Commit,
            crate::db::query::StatementResultKind::Rollback => MysqlStatementKind::Rollback,
            crate::db::query::StatementResultKind::Use => MysqlStatementKind::Use,
            crate::db::query::StatementResultKind::Call => MysqlStatementKind::Call,
            crate::db::query::StatementResultKind::Empty
            | crate::db::query::StatementResultKind::Exec
            | crate::db::query::StatementResultKind::Ddl => MysqlStatementKind::Ddl,
        }
    }

    pub(crate) fn row_to_strings(row: &Row, column_count: usize) -> Vec<String> {
        let mut row_values = Vec::with_capacity(column_count);
        for index in 0..column_count {
            let value = row
                .as_ref(index)
                .map(Self::value_to_string)
                .unwrap_or_else(QueryCell::null_result_text);
            row_values.push(value);
        }
        row_values
    }

    #[cfg(test)]
    pub(crate) fn is_select_statement(sql: &str) -> bool {
        matches!(Self::classify_statement(sql), MysqlStatementKind::Select)
    }

    pub(crate) fn is_select_statement_for_db_type(db_type: DatabaseType, sql: &str) -> bool {
        matches!(
            Self::classify_statement_for_db_type(db_type, sql),
            MysqlStatementKind::Select
        )
    }

    #[cfg(test)]
    pub(crate) fn is_displayable_select_statement(sql: &str) -> bool {
        Self::is_displayable_select_statement_for_db_type(DatabaseType::MySQL, sql)
    }

    pub(crate) fn is_displayable_select_statement_for_db_type(
        db_type: DatabaseType,
        sql: &str,
    ) -> bool {
        Self::is_select_statement_for_db_type(db_type, sql)
            && !Self::select_statement_targets_non_result_destination_for_db_type(db_type, sql)
    }

    fn select_statement_targets_non_result_destination_for_db_type(
        db_type: DatabaseType,
        sql: &str,
    ) -> bool {
        let analysis =
            crate::db::sql_classification::SqlStatementAnalysis::new_for_db_type(db_type, sql);
        let words = analysis.words();
        let select_index = match Self::result_producing_select_word_index(words) {
            Some(index) => index,
            None if sql.trim_start().starts_with('(') => 0,
            None => return false,
        };

        words
            .iter()
            .skip(select_index.saturating_add(1))
            .any(|word| word == "INTO")
    }

    fn result_producing_select_word_index(words: &[String]) -> Option<usize> {
        match words.first().map(String::as_str) {
            Some("SELECT") => Some(0),
            Some("WITH") => words.iter().position(|word| word == "SELECT"),
            Some("TABLE" | "VALUES") => Some(0),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn is_use_statement(sql: &str) -> bool {
        Self::is_use_statement_for_db_type(DatabaseType::MySQL, sql)
    }

    pub(crate) fn is_use_statement_for_db_type(db_type: DatabaseType, sql: &str) -> bool {
        matches!(
            Self::classify_statement_for_db_type(db_type, sql),
            MysqlStatementKind::Use
        )
    }

    #[cfg(test)]
    pub(crate) fn use_statement_database_name(sql: &str) -> Option<String> {
        Self::use_statement_database_name_for_db_type(DatabaseType::MySQL, sql)
    }

    pub(crate) fn use_statement_database_name_for_db_type(
        db_type: DatabaseType,
        sql: &str,
    ) -> Option<String> {
        if !Self::is_use_statement_for_db_type(db_type, sql) {
            return None;
        }
        Some(Self::extract_use_database_name(sql.trim()))
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
    }

    fn transaction_control_sql_to_execute<'a>(sql: &'a str, keyword: &'static str) -> &'a str {
        match keyword {
            "COMMIT" if QueryExecutor::is_plain_commit(sql) => keyword,
            "ROLLBACK" if QueryExecutor::is_plain_rollback(sql) => keyword,
            _ => sql,
        }
    }

    /// Classify a MySQL/MariaDB wire column type for SQL literal generation.
    ///
    /// Exhaustive on purpose: when `ColumnType` gains a variant the compiler
    /// forces a decision here instead of silently defaulting.
    ///
    /// The protocol cannot separate TEXT from BLOB, or VARCHAR from VARBINARY —
    /// both pairs share a type code and differ only by charset. That does not
    /// matter here: `Binary` and `String` both render as a quoted literal on
    /// this backend, because `value_to_string` has already put the bytes
    /// through `String::from_utf8_lossy`.
    pub(crate) fn mysql_value_kind(column_type: mysql::consts::ColumnType) -> SqlValueKind {
        use mysql::consts::ColumnType as MysqlColumnType;
        match column_type {
            MysqlColumnType::MYSQL_TYPE_TINY
            | MysqlColumnType::MYSQL_TYPE_SHORT
            | MysqlColumnType::MYSQL_TYPE_LONG
            | MysqlColumnType::MYSQL_TYPE_LONGLONG
            | MysqlColumnType::MYSQL_TYPE_INT24
            | MysqlColumnType::MYSQL_TYPE_FLOAT
            | MysqlColumnType::MYSQL_TYPE_DOUBLE
            | MysqlColumnType::MYSQL_TYPE_DECIMAL
            | MysqlColumnType::MYSQL_TYPE_NEWDECIMAL
            | MysqlColumnType::MYSQL_TYPE_YEAR => SqlValueKind::Number,
            MysqlColumnType::MYSQL_TYPE_VARCHAR
            | MysqlColumnType::MYSQL_TYPE_VAR_STRING
            | MysqlColumnType::MYSQL_TYPE_STRING
            | MysqlColumnType::MYSQL_TYPE_JSON
            | MysqlColumnType::MYSQL_TYPE_ENUM
            | MysqlColumnType::MYSQL_TYPE_SET => SqlValueKind::String,
            MysqlColumnType::MYSQL_TYPE_DATE
            | MysqlColumnType::MYSQL_TYPE_NEWDATE
            | MysqlColumnType::MYSQL_TYPE_DATETIME
            | MysqlColumnType::MYSQL_TYPE_DATETIME2
            | MysqlColumnType::MYSQL_TYPE_TIMESTAMP
            | MysqlColumnType::MYSQL_TYPE_TIMESTAMP2
            | MysqlColumnType::MYSQL_TYPE_TIME
            | MysqlColumnType::MYSQL_TYPE_TIME2 => SqlValueKind::Temporal,
            MysqlColumnType::MYSQL_TYPE_BLOB
            | MysqlColumnType::MYSQL_TYPE_TINY_BLOB
            | MysqlColumnType::MYSQL_TYPE_MEDIUM_BLOB
            | MysqlColumnType::MYSQL_TYPE_LONG_BLOB => SqlValueKind::Binary,
            MysqlColumnType::MYSQL_TYPE_BIT
            | MysqlColumnType::MYSQL_TYPE_GEOMETRY
            | MysqlColumnType::MYSQL_TYPE_NULL
            | MysqlColumnType::MYSQL_TYPE_TYPED_ARRAY
            | MysqlColumnType::MYSQL_TYPE_UNKNOWN => SqlValueKind::Unknown,
        }
    }

    fn value_to_string(value: &MysqlValue) -> String {
        match value {
            MysqlValue::NULL => QueryCell::null_result_text(),
            MysqlValue::Bytes(bytes) => String::from_utf8_lossy(bytes).into_owned(),
            MysqlValue::Int(number) => number.to_string(),
            MysqlValue::UInt(number) => number.to_string(),
            MysqlValue::Float(number) => number.to_string(),
            MysqlValue::Double(number) => number.to_string(),
            MysqlValue::Date(year, month, day, hour, minute, second, micros) => {
                if *year == 0 && *month == 0 && *day == 0 {
                    return "0000-00-00".to_string();
                }
                if *hour == 0 && *minute == 0 && *second == 0 && *micros == 0 {
                    format!("{year:04}-{month:02}-{day:02}")
                } else if *micros == 0 {
                    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
                } else {
                    format!(
                        "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{micros:06}"
                    )
                }
            }
            MysqlValue::Time(is_negative, days, hours, minutes, seconds, micros) => {
                let sign = if *is_negative { "-" } else { "" };
                let total_hours = days.saturating_mul(24).saturating_add(*hours as u32);
                if *micros == 0 {
                    format!("{sign}{total_hours:02}:{minutes:02}:{seconds:02}")
                } else {
                    format!("{sign}{total_hours:02}:{minutes:02}:{seconds:02}.{micros:06}")
                }
            }
        }
    }

    pub fn execute<C: Queryable>(conn: &mut C, sql: &str) -> Result<Vec<QueryResult>, MysqlError> {
        Self::execute_for_db_type(conn, sql, DatabaseType::MySQL)
    }

    pub fn execute_for_db_type<C: Queryable>(
        conn: &mut C,
        sql: &str,
        db_type: DatabaseType,
    ) -> Result<Vec<QueryResult>, MysqlError> {
        Self::execute_for_db_type_with_cancel(conn, sql, db_type, || false)
    }

    pub fn execute_for_db_type_with_cancel<C, F>(
        conn: &mut C,
        sql: &str,
        db_type: DatabaseType,
        mut is_cancelled: F,
    ) -> Result<Vec<QueryResult>, MysqlError>
    where
        C: Queryable,
        F: FnMut() -> bool,
    {
        let trimmed = sql.trim();
        match Self::classify_statement_for_db_type(db_type, trimmed) {
            MysqlStatementKind::Select => Ok(vec![Self::execute_select_with_cancel(
                conn,
                sql,
                &mut is_cancelled,
            )?]),
            MysqlStatementKind::Dml => Ok(vec![Self::execute_dml(conn, sql)?]),
            MysqlStatementKind::Commit => {
                let start = Instant::now();
                conn.query_drop(Self::transaction_control_sql_to_execute(sql, "COMMIT"))?;
                Ok(vec![QueryResult::new_non_select_success(
                    sql,
                    result_messages::COMMIT_COMPLETE,
                    start.elapsed(),
                )])
            }
            MysqlStatementKind::Rollback => {
                let start = Instant::now();
                conn.query_drop(Self::transaction_control_sql_to_execute(sql, "ROLLBACK"))?;
                Ok(vec![QueryResult::new_non_select_success(
                    sql,
                    result_messages::ROLLBACK_COMPLETE,
                    start.elapsed(),
                )])
            }
            MysqlStatementKind::Use => {
                let start = Instant::now();
                conn.query_drop(sql)?;
                let db_name = Self::extract_use_database_name(trimmed);
                Ok(vec![QueryResult::new_non_select_success(
                    sql,
                    Self::current_database_changed_message(&db_name),
                    start.elapsed(),
                )])
            }
            MysqlStatementKind::Call => Self::execute_call(conn, sql),
            MysqlStatementKind::Ddl => Ok(vec![Self::execute_ddl(conn, sql)?]),
        }
    }

    fn execute_select<C: Queryable>(conn: &mut C, sql: &str) -> Result<QueryResult, MysqlError> {
        Self::execute_select_with_cancel(conn, sql, &mut || false)
    }

    fn cancelled_select_result(
        sql: &str,
        columns: Vec<ColumnInfo>,
        rows: Vec<Vec<String>>,
        execution_time: Duration,
    ) -> QueryResult {
        QueryResult {
            sql: sql.to_string(),
            row_count: rows.len(),
            columns,
            rows,
            execution_time,
            message: result_messages::QUERY_CANCELLED.to_string(),
            is_select: true,
            success: false,
        }
    }

    fn execute_select_with_cancel<C, F>(
        conn: &mut C,
        sql: &str,
        is_cancelled: &mut F,
    ) -> Result<QueryResult, MysqlError>
    where
        C: Queryable,
        F: FnMut() -> bool,
    {
        let start = Instant::now();
        let result = conn.query_iter(sql)?;

        let columns: Vec<ColumnInfo> = result
            .columns()
            .as_ref()
            .iter()
            .map(|col| ColumnInfo {
                name: col.name_str().to_string(),
                data_type: format!("{:?}", col.column_type()),
                kind: Self::mysql_value_kind(col.column_type()),
            })
            .collect();

        if is_cancelled() {
            return Ok(Self::cancelled_select_result(
                sql,
                columns,
                Vec::new(),
                start.elapsed(),
            ));
        }

        if columns.is_empty() {
            for row_result in result {
                if is_cancelled() {
                    return Ok(Self::cancelled_select_result(
                        sql,
                        Vec::new(),
                        Vec::new(),
                        start.elapsed(),
                    ));
                }
                let _: Row = row_result?;
            }
            return Ok(QueryResult::new_non_select_success(
                sql,
                result_messages::STATEMENT_EXECUTED,
                start.elapsed(),
            ));
        }

        let mut rows: Vec<Vec<String>> = Vec::new();
        for row_result in result {
            if is_cancelled() {
                return Ok(Self::cancelled_select_result(
                    sql,
                    columns,
                    rows,
                    start.elapsed(),
                ));
            }
            let row: Row = row_result?;
            if is_cancelled() {
                return Ok(Self::cancelled_select_result(
                    sql,
                    columns,
                    rows,
                    start.elapsed(),
                ));
            }
            rows.push(Self::row_to_strings(&row, columns.len()));
        }

        Ok(QueryResult::new_select(sql, columns, rows, start.elapsed()))
    }

    pub fn execute_select_streaming<C, F, G>(
        conn: &mut C,
        sql: &str,
        on_select_start: &mut F,
        on_row: &mut G,
    ) -> Result<(QueryResult, bool), MysqlError>
    where
        C: Queryable,
        F: FnMut(&[ColumnInfo]),
        G: FnMut(Vec<String>) -> bool,
    {
        let start = Instant::now();
        let mut result = conn.query_iter(sql)?;

        let columns: Vec<ColumnInfo> = result
            .columns()
            .as_ref()
            .iter()
            .map(|col| ColumnInfo {
                name: col.name_str().to_string(),
                data_type: format!("{:?}", col.column_type()),
                kind: Self::mysql_value_kind(col.column_type()),
            })
            .collect();

        on_select_start(&columns);

        let mut row_count: usize = 0;
        let mut cancelled = false;

        for row_result in result.by_ref() {
            let row: Row = row_result?;
            row_count += 1;
            if !on_row(Self::row_to_strings(&row, columns.len())) {
                cancelled = true;
                break;
            }
        }

        // The MySQL driver drains unread rows/result sets when QueryResult is
        // dropped. This non-lazy helper has no cancel socket/context, so the
        // only protocol-safe choice is to finish driver cleanup before callers
        // consider reuse. Cancellable lazy fetch uses the separate UI worker
        // path that issues KILL QUERY and may discard the physical session.
        drop(result);

        let query_result =
            QueryResult::new_select_streamed(sql, columns, row_count, start.elapsed());
        Ok((query_result, cancelled))
    }

    fn execute_dml<C: Queryable>(conn: &mut C, sql: &str) -> Result<QueryResult, MysqlError> {
        let start = Instant::now();
        let mut query_result = conn.query_iter(sql)?;
        let fallback_affected_rows = query_result.affected_rows();
        let trimmed = sql.trim();
        let stmt_type = if QueryExecutor::leading_keyword(trimmed)
            .as_deref()
            .is_some_and(|keyword| keyword.eq_ignore_ascii_case("WITH"))
        {
            "DML".to_string()
        } else {
            trimmed
                .split_whitespace()
                .next()
                .unwrap_or("DML")
                .to_ascii_uppercase()
        };

        let mut snapshots = Vec::new();
        while let Some(mut result_set) = query_result.iter() {
            let columns: Vec<ColumnInfo> = result_set
                .columns()
                .as_ref()
                .iter()
                .map(|col| ColumnInfo {
                    name: col.name_str().to_string(),
                    data_type: format!("{:?}", col.column_type()),
                    kind: Self::mysql_value_kind(col.column_type()),
                })
                .collect();

            let mut rows = Vec::new();
            for row_result in result_set.by_ref() {
                let row: Row = row_result?;
                if !columns.is_empty() {
                    rows.push(Self::row_to_strings(&row, columns.len()));
                }
            }

            snapshots.push(MysqlResultSetSnapshot {
                columns,
                rows,
                affected_rows: result_set.affected_rows(),
                info: result_set.info_str().into_owned(),
            });
        }

        Ok(Self::dml_result_from_snapshots(
            sql,
            snapshots,
            fallback_affected_rows,
            start.elapsed(),
            &stmt_type,
        ))
    }

    fn dml_result_from_snapshots(
        sql: &str,
        snapshots: Vec<MysqlResultSetSnapshot>,
        fallback_affected_rows: u64,
        execution_time: Duration,
        stmt_type: &str,
    ) -> QueryResult {
        let affected_rows = snapshots
            .iter()
            .map(|snapshot| snapshot.affected_rows)
            .sum::<u64>();
        let affected_rows = affected_rows.max(fallback_affected_rows);
        let mut returning: Option<(Vec<ColumnInfo>, Vec<Vec<String>>)> = None;

        for snapshot in snapshots {
            if snapshot.columns.is_empty() {
                continue;
            }
            match &mut returning {
                Some((columns, rows)) if columns.len() == snapshot.columns.len() => {
                    rows.extend(snapshot.rows);
                }
                Some(_) => {}
                None => {
                    returning = Some((snapshot.columns, snapshot.rows));
                }
            }
        }

        if let Some((columns, rows)) = returning {
            return QueryResult::new_dml_returning(
                sql,
                columns,
                rows,
                affected_rows,
                execution_time,
                stmt_type,
            );
        }

        QueryResult::new_dml(sql, affected_rows, execution_time, stmt_type)
    }

    fn execute_ddl<C: Queryable>(conn: &mut C, sql: &str) -> Result<QueryResult, MysqlError> {
        let start = Instant::now();
        conn.query_drop(sql)?;
        Ok(QueryResult::new_non_select_success(
            sql,
            QueryExecutor::ddl_message(sql),
            start.elapsed(),
        ))
    }

    fn call_result_from_snapshot(
        sql: &str,
        snapshot: MysqlResultSetSnapshot,
        execution_time: Duration,
    ) -> Option<QueryResult> {
        if !snapshot.columns.is_empty() {
            return Some(QueryResult::new_select(
                sql,
                snapshot.columns,
                snapshot.rows,
                execution_time,
            ));
        }

        let info = snapshot.info.trim();
        if snapshot.affected_rows > 0 {
            let mut result =
                QueryResult::new_dml(sql, snapshot.affected_rows, execution_time, "CALL");
            if !info.is_empty() {
                result.message = format!("{} | {}", result.message, info);
            }
            return Some(result);
        }

        if !info.is_empty() {
            return Some(QueryResult::new_non_select_success(
                sql,
                format!("{} | {}", result_messages::CALL_EXECUTED, info),
                execution_time,
            ));
        }

        None
    }

    fn default_call_result(sql: &str, execution_time: Duration) -> QueryResult {
        QueryResult::new_non_select_success(sql, result_messages::CALL_EXECUTED, execution_time)
    }

    fn materialize_call_results(
        sql: &str,
        snapshots: Vec<MysqlResultSetSnapshot>,
        execution_time: Duration,
    ) -> Vec<QueryResult> {
        let mut results = snapshots
            .into_iter()
            .filter_map(|snapshot| Self::call_result_from_snapshot(sql, snapshot, execution_time))
            .collect::<Vec<_>>();
        if results.is_empty() {
            results.push(Self::default_call_result(sql, execution_time));
        }
        results
    }

    fn execute_call<C: Queryable>(conn: &mut C, sql: &str) -> Result<Vec<QueryResult>, MysqlError> {
        // CALL may return multiple select and non-select result sets.
        let start = Instant::now();
        let mut query_result = conn.query_iter(sql)?;
        let mut snapshots = Vec::new();

        while let Some(mut result_set) = query_result.iter() {
            let columns: Vec<ColumnInfo> = result_set
                .columns()
                .as_ref()
                .iter()
                .map(|col| ColumnInfo {
                    name: col.name_str().to_string(),
                    data_type: format!("{:?}", col.column_type()),
                    kind: Self::mysql_value_kind(col.column_type()),
                })
                .collect();

            let mut rows = Vec::new();
            for row_result in result_set.by_ref() {
                let row: Row = row_result?;
                if !columns.is_empty() {
                    rows.push(Self::row_to_strings(&row, columns.len()));
                }
            }

            snapshots.push(MysqlResultSetSnapshot {
                columns,
                rows,
                affected_rows: result_set.affected_rows(),
                info: result_set.info_str().into_owned(),
            });
        }

        Ok(Self::materialize_call_results(
            sql,
            snapshots,
            start.elapsed(),
        ))
    }

    pub fn execute_batch<C>(
        conn: &mut C,
        statements: &[String],
    ) -> Vec<Result<QueryResult, MysqlError>>
    where
        C: Queryable,
    {
        let mut results = Vec::new();
        for stmt in statements {
            let trimmed = stmt.trim();
            if trimmed.is_empty() {
                continue;
            }
            match Self::execute(conn, trimmed) {
                Ok(statement_results) => {
                    results.extend(statement_results.into_iter().map(Ok));
                }
                Err(err) => results.push(Err(err)),
            }
        }
        results
    }

    fn build_explain_sql(sql: &str) -> String {
        let normalized = QueryExecutor::normalize_sql_for_execute(sql);
        match QueryExecutor::leading_keyword(&normalized).as_deref() {
            Some("EXPLAIN") | Some("DESCRIBE") | Some("DESC") => normalized,
            _ => format!("EXPLAIN {}", normalized),
        }
    }

    /// Run EXPLAIN and hand back the server's own result, columns and all.
    ///
    /// Nothing is reshaped here: classic `EXPLAIN` has no parent column, so the
    /// caller renders the rows flat rather than inventing a tree out of
    /// `id`/`select_type`.
    pub fn get_explain_plan(conn: &mut Conn, sql: &str) -> Result<QueryResult, MysqlError> {
        let explain_sql = Self::build_explain_sql(sql);
        Self::execute_select(conn, &explain_sql)
    }

    fn build_cancel_opts(info: &ConnectionInfo) -> mysql::OptsBuilder {
        crate::db::DatabaseConnection::build_mysql_opts_without_database(info)
            .tcp_connect_timeout(Some(MYSQL_CANCEL_IO_TIMEOUT))
            .read_timeout(Some(MYSQL_CANCEL_IO_TIMEOUT))
            .write_timeout(Some(MYSQL_CANCEL_IO_TIMEOUT))
    }

    fn normalize_cancel_result(result: Result<(), MysqlError>) -> Result<(), MysqlError> {
        match result {
            Err(MysqlError::MySqlError(error)) if error.code == 1094 => Ok(()),
            result => result,
        }
    }

    pub fn cancel_running_query(
        info: &ConnectionInfo,
        connection_id: u32,
    ) -> Result<(), MysqlError> {
        let opts = Self::build_cancel_opts(info);
        let mut cancel_conn = mysql::Conn::new(opts)?;
        let kill_sql = format!("KILL QUERY {connection_id}");
        Self::normalize_cancel_result(cancel_conn.query_drop(kill_sql.as_str()))
    }

    pub fn cancel_connection(info: &ConnectionInfo, connection_id: u32) -> Result<(), MysqlError> {
        let opts = Self::build_cancel_opts(info);
        let mut cancel_conn = mysql::Conn::new(opts)?;
        let kill_sql = format!("KILL CONNECTION {connection_id}");
        Self::normalize_cancel_result(cancel_conn.query_drop(kill_sql.as_str()))
    }

    fn current_database_changed_message(database: &str) -> String {
        result_messages::current_scope_changed("database", database)
    }

    fn skip_to_next_line(bytes: &[u8], mut index: usize) -> usize {
        while index < bytes.len() {
            let byte = bytes[index];
            index += 1;
            if byte == b'\n' || byte == b'\r' {
                break;
            }
        }
        index
    }

    fn skip_use_statement_trivia(source: &str, mut index: usize) -> usize {
        let bytes = source.as_bytes();

        loop {
            while bytes
                .get(index)
                .is_some_and(|byte| byte.is_ascii_whitespace())
            {
                index += 1;
            }

            if bytes.get(index) == Some(&b'/') && bytes.get(index + 1) == Some(&b'*') {
                index += 2;
                let mut closed = false;
                while index + 1 < bytes.len() {
                    if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                        index += 2;
                        closed = true;
                        break;
                    }
                    index += 1;
                }
                if !closed {
                    return bytes.len();
                }
                continue;
            }

            if bytes.get(index) == Some(&b'#') {
                index = Self::skip_to_next_line(bytes, index + 1);
                continue;
            }

            if sql_text::is_mysql_dash_comment_start(bytes, index) {
                index = Self::skip_to_next_line(bytes, index + 2);
                continue;
            }

            break;
        }

        index
    }

    /// Extract the database name from a `USE <db>` statement for display purposes.
    /// Handles backtick-quoted identifiers, including names containing spaces.
    fn extract_use_database_name(trimmed_use_sql: &str) -> String {
        let bytes = trimmed_use_sql.as_bytes();
        let mut index = Self::skip_use_statement_trivia(trimmed_use_sql, 0);

        if bytes
            .get(index..index.saturating_add("USE".len()))
            .is_some_and(|slice| slice.eq_ignore_ascii_case(b"USE"))
        {
            index = index.saturating_add("USE".len());
        }

        index = Self::skip_use_statement_trivia(trimmed_use_sql, index);

        Self::leading_identifier_token(trimmed_use_sql.get(index..).unwrap_or(""))
    }

    /// First identifier token of `after`, honoring backtick quoting with ``
    /// escapes; unquoted names end at whitespace or `;`.
    fn leading_identifier_token(after: &str) -> String {
        let after = after.trim_start();
        if after.starts_with('`') {
            // Backtick-quoted identifier: scan for the closing backtick,
            // treating `` as an escaped backtick.
            let bytes = after.as_bytes();
            let mut idx = 1usize;
            while idx < bytes.len() {
                if bytes[idx] == b'`' {
                    if bytes.get(idx + 1) == Some(&b'`') {
                        idx += 2;
                    } else {
                        break;
                    }
                } else {
                    idx += 1;
                }
            }
            after.get(1..idx).unwrap_or(after).replace("``", "`")
        } else {
            // Unquoted: take the first whitespace/semicolon-delimited token.
            after
                .split(|c: char| c.is_ascii_whitespace() || c == ';')
                .next()
                .unwrap_or("")
                .to_string()
        }
    }

    fn strip_leading_keyword<'a>(sql: &'a str, keyword: &str) -> Option<&'a str> {
        let candidate = sql.trim_start();
        let bytes = candidate.as_bytes();
        if bytes.len() <= keyword.len() {
            return None;
        }
        if !bytes[..keyword.len()].eq_ignore_ascii_case(keyword.as_bytes()) {
            return None;
        }
        if !bytes[keyword.len()].is_ascii_whitespace() {
            return None;
        }
        Some(&candidate[keyword.len()..])
    }

    /// Database name dropped by a `DROP DATABASE`/`DROP SCHEMA` statement,
    /// or `None` when the statement is not one.
    pub(crate) fn drop_database_statement_database_name_for_db_type(
        _db_type: DatabaseType,
        sql: &str,
    ) -> Option<String> {
        let rest = Self::strip_leading_keyword(sql, "DROP")?;
        let rest = Self::strip_leading_keyword(rest, "DATABASE")
            .or_else(|| Self::strip_leading_keyword(rest, "SCHEMA"))?;
        let rest = match Self::strip_leading_keyword(rest, "IF") {
            Some(after_if) => Self::strip_leading_keyword(after_if, "EXISTS")?,
            None => rest,
        };
        let name = Self::leading_identifier_token(rest).trim().to_string();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    }

    pub fn is_lock_wait_timeout_error(err: &MysqlError) -> bool {
        let lowered = err.to_string().to_ascii_lowercase();
        matches!(err, MysqlError::MySqlError(server_err) if server_err.code == 1205)
            || lowered.contains("lock wait timeout exceeded")
    }

    /// Check if a MySQL error is a timeout/cancelled error.
    pub fn is_timeout_error(err: &MysqlError) -> bool {
        let lowered = err.to_string().to_ascii_lowercase();
        matches!(err, MysqlError::MySqlError(server_err) if matches!(server_err.code, 3024 | 1969))
            || lowered.contains("er_query_timeout")
            || lowered.contains("max_execution_time")
            || lowered.contains("max_statement_time")
            || lowered.contains("max statement time exceeded")
            || lowered.contains("maximum statement execution time exceeded")
            || lowered.contains("query timed out")
            || Self::is_lock_wait_timeout_error(err)
    }

    pub fn is_cancel_error(err: &MysqlError) -> bool {
        if Self::is_timeout_error(err) {
            return false;
        }

        let lowered = err.to_string().to_ascii_lowercase();
        matches!(err, MysqlError::MySqlError(server_err) if server_err.code == 1317)
            || lowered.contains("query execution was interrupted")
            || lowered.contains("query was killed")
    }
}

// ---------------------------------------------------------------------------
// MySQL Object Browser
// ---------------------------------------------------------------------------

pub struct MysqlTableColumnDetail {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub default_value: Option<String>,
    pub is_primary_key: bool,
    pub extra: String,
}

impl MysqlObjectBrowser {
    fn create_ddl_column_names(object_type: &str) -> &'static [&'static str] {
        match object_type {
            "TABLE" | "VIEW" => &["Create Table", "Create View"],
            "PROCEDURE" => &["Create Procedure"],
            "FUNCTION" => &["Create Function"],
            "TRIGGER" => &["SQL Original Statement", "Create Trigger"],
            "EVENT" => &["Create Event"],
            "SEQUENCE" => &["Create Sequence", "Create Table"],
            _ => &[],
        }
    }

    fn optional_schema_param(schema_name: Option<&str>) -> Option<String> {
        schema_name
            .map(str::trim)
            .filter(|schema| !schema.is_empty())
            .map(ToOwned::to_owned)
    }

    fn escape_identifier(identifier: &str) -> String {
        identifier.replace('`', "``")
    }

    fn quoted_identifier(identifier: &str) -> String {
        format!("`{}`", Self::escape_identifier(identifier))
    }

    fn skip_ascii_whitespace(source: &str, mut index: usize) -> usize {
        let bytes = source.as_bytes();
        while bytes
            .get(index)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            index += 1;
        }
        index
    }

    fn append_comment_free_segment(cleaned: &mut String, source: &str, start: usize, end: usize) {
        if start >= end {
            return;
        }

        if let Some(segment) = source.get(start..end) {
            cleaned.push_str(segment);
        }
    }

    fn ensure_comment_gap(cleaned: &mut String) {
        if cleaned.chars().last().is_some_and(|ch| !ch.is_whitespace()) {
            cleaned.push(' ');
        }
    }

    /// Strip MySQL/MariaDB inline comments from `source`, honouring single-
    /// quoted, double-quoted and backtick-quoted literals.
    ///
    /// Comments are replaced with at most one separating space so adjacent
    /// tokens remain parseable after removal.
    fn strip_mysql_inline_comments(source: &str) -> String {
        let bytes = source.as_bytes();
        let mut cleaned = String::with_capacity(source.len());
        let mut segment_start = 0usize;
        let mut index = 0usize;
        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut in_backtick = false;

        while index < bytes.len() {
            let byte = bytes[index];
            let next = bytes.get(index + 1).copied();

            if in_single_quote {
                if byte == b'\\' && next.is_some() {
                    index += 2;
                    continue;
                }
                if byte == b'\'' {
                    if next == Some(b'\'') {
                        index += 2;
                        continue;
                    }
                    in_single_quote = false;
                }
                index += 1;
                continue;
            }

            if in_double_quote {
                if byte == b'\\' && next.is_some() {
                    index += 2;
                    continue;
                }
                if byte == b'"' {
                    if next == Some(b'"') {
                        index += 2;
                        continue;
                    }
                    in_double_quote = false;
                }
                index += 1;
                continue;
            }

            if in_backtick {
                if byte == b'`' {
                    if next == Some(b'`') {
                        index += 2;
                        continue;
                    }
                    in_backtick = false;
                }
                index += 1;
                continue;
            }

            match byte {
                b'\'' => {
                    in_single_quote = true;
                    index += 1;
                    continue;
                }
                b'"' => {
                    in_double_quote = true;
                    index += 1;
                    continue;
                }
                b'`' => {
                    in_backtick = true;
                    index += 1;
                    continue;
                }
                b'#' => {
                    Self::append_comment_free_segment(&mut cleaned, source, segment_start, index);
                    Self::ensure_comment_gap(&mut cleaned);
                    while index < bytes.len() && bytes[index] != b'\n' {
                        index += 1;
                    }
                    segment_start = Self::skip_ascii_whitespace(source, index);
                    continue;
                }
                b'-' if sql_text::is_mysql_dash_comment_start(bytes, index) => {
                    Self::append_comment_free_segment(&mut cleaned, source, segment_start, index);
                    Self::ensure_comment_gap(&mut cleaned);
                    while index < bytes.len() && bytes[index] != b'\n' {
                        index += 1;
                    }
                    segment_start = Self::skip_ascii_whitespace(source, index);
                    continue;
                }
                b'/' if next == Some(b'*') => {
                    Self::append_comment_free_segment(&mut cleaned, source, segment_start, index);
                    Self::ensure_comment_gap(&mut cleaned);
                    index += 2;
                    let mut closed = false;
                    while index + 1 < bytes.len() {
                        if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                            index += 2;
                            closed = true;
                            break;
                        }
                        index += 1;
                    }
                    if !closed {
                        index = bytes.len();
                    }
                    segment_start = Self::skip_ascii_whitespace(source, index);
                    continue;
                }
                _ => {}
            }

            index += 1;
        }

        Self::append_comment_free_segment(&mut cleaned, source, segment_start, source.len());
        cleaned
    }

    fn unquote_identifier(identifier: &str) -> String {
        let trimmed = identifier.trim();
        if let Some(inner) = trimmed
            .strip_prefix('`')
            .and_then(|value| value.strip_suffix('`'))
        {
            return inner.replace("``", "`");
        }
        trimmed.to_string()
    }

    fn parse_identifier_segment_end(source: &str, start: usize) -> Option<usize> {
        let bytes = source.as_bytes();
        match bytes.get(start).copied() {
            Some(b'`') => {
                let mut index = start + 1;
                while index < bytes.len() {
                    if bytes[index] == b'`' {
                        if bytes.get(index + 1) == Some(&b'`') {
                            index += 2;
                            continue;
                        }
                        return Some(index + 1);
                    }
                    index += 1;
                }
                None
            }
            Some(_) => {
                let mut index = start;
                while let Some(byte) = bytes.get(index).copied() {
                    if byte.is_ascii_whitespace() || matches!(byte, b'.' | b'(' | b')' | b',') {
                        break;
                    }
                    index += 1;
                }
                if index > start {
                    Some(index)
                } else {
                    None
                }
            }
            None => None,
        }
    }

    fn parse_identifier_path_end(source: &str, start: usize) -> Option<usize> {
        let mut index = Self::skip_ascii_whitespace(source, start);
        let segment_end = Self::parse_identifier_segment_end(source, index)?;
        index = segment_end;

        loop {
            let bytes = source.as_bytes();
            if bytes.get(index) != Some(&b'.') {
                break;
            }
            let next_segment_start = Self::skip_ascii_whitespace(source, index + 1);
            let next_segment_end = Self::parse_identifier_segment_end(source, next_segment_start)?;
            index = next_segment_end;
        }

        Some(index)
    }

    fn keyword_matches_at(source: &str, index: usize, keyword: &str) -> bool {
        let keyword_len = keyword.len();
        let Some(slice) = source.get(index..index.saturating_add(keyword_len)) else {
            return false;
        };
        if !slice.eq_ignore_ascii_case(keyword) {
            return false;
        }

        let bytes = source.as_bytes();
        let prev = index.checked_sub(1).and_then(|idx| bytes.get(idx)).copied();
        let next = bytes.get(index.saturating_add(keyword_len)).copied();
        let is_ident = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$');

        !prev.is_some_and(is_ident) && !next.is_some_and(is_ident)
    }

    fn find_keyword_at_top_level(source: &str, keyword: &str, start: usize) -> Option<usize> {
        let bytes = source.as_bytes();
        let mut index = start.min(bytes.len());
        let mut depth = 0usize;
        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut in_backtick = false;
        let mut in_line_comment = false;
        let mut in_block_comment = false;

        while index < bytes.len() {
            let byte = bytes[index];
            let next = bytes.get(index + 1).copied();

            if in_line_comment {
                if byte == b'\n' {
                    in_line_comment = false;
                }
                index += 1;
                continue;
            }

            if in_block_comment {
                if byte == b'*' && next == Some(b'/') {
                    in_block_comment = false;
                    index += 2;
                } else {
                    index += 1;
                }
                continue;
            }

            if in_single_quote {
                if byte == b'\\' && next.is_some() {
                    index += 2;
                    continue;
                }
                if byte == b'\'' {
                    if next == Some(b'\'') {
                        index += 2;
                        continue;
                    }
                    in_single_quote = false;
                }
                index += 1;
                continue;
            }

            if in_double_quote {
                if byte == b'\\' && next.is_some() {
                    index += 2;
                    continue;
                }
                if byte == b'"' {
                    if next == Some(b'"') {
                        index += 2;
                        continue;
                    }
                    in_double_quote = false;
                }
                index += 1;
                continue;
            }

            if in_backtick {
                if byte == b'`' {
                    if next == Some(b'`') {
                        index += 2;
                        continue;
                    }
                    in_backtick = false;
                }
                index += 1;
                continue;
            }

            if sql_text::is_mysql_dash_comment_start(bytes, index) {
                in_line_comment = true;
                index += 2;
                continue;
            }

            if byte == b'#' {
                in_line_comment = true;
                index += 1;
                continue;
            }

            if byte == b'/' && next == Some(b'*') {
                in_block_comment = true;
                index += 2;
                continue;
            }

            match byte {
                b'\'' => {
                    in_single_quote = true;
                    index += 1;
                    continue;
                }
                b'"' => {
                    in_double_quote = true;
                    index += 1;
                    continue;
                }
                b'`' => {
                    in_backtick = true;
                    index += 1;
                    continue;
                }
                b'(' => {
                    depth += 1;
                }
                b')' => {
                    depth = depth.saturating_sub(1);
                }
                _ => {}
            }

            if depth == 0 && Self::keyword_matches_at(source, index, keyword) {
                return Some(index);
            }

            index += 1;
        }

        None
    }

    fn find_matching_paren(source: &str, open_index: usize) -> Option<usize> {
        let bytes = source.as_bytes();
        if bytes.get(open_index) != Some(&b'(') {
            return None;
        }

        let mut index = open_index;
        let mut depth = 0usize;
        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut in_backtick = false;
        let mut in_line_comment = false;
        let mut in_block_comment = false;

        while index < bytes.len() {
            let byte = bytes[index];
            let next = bytes.get(index + 1).copied();

            if in_line_comment {
                if byte == b'\n' {
                    in_line_comment = false;
                }
                index += 1;
                continue;
            }

            if in_block_comment {
                if byte == b'*' && next == Some(b'/') {
                    in_block_comment = false;
                    index += 2;
                } else {
                    index += 1;
                }
                continue;
            }

            if in_single_quote {
                if byte == b'\\' && next.is_some() {
                    index += 2;
                    continue;
                }
                if byte == b'\'' {
                    if next == Some(b'\'') {
                        index += 2;
                        continue;
                    }
                    in_single_quote = false;
                }
                index += 1;
                continue;
            }

            if in_double_quote {
                if byte == b'\\' && next.is_some() {
                    index += 2;
                    continue;
                }
                if byte == b'"' {
                    if next == Some(b'"') {
                        index += 2;
                        continue;
                    }
                    in_double_quote = false;
                }
                index += 1;
                continue;
            }

            if in_backtick {
                if byte == b'`' {
                    if next == Some(b'`') {
                        index += 2;
                        continue;
                    }
                    in_backtick = false;
                }
                index += 1;
                continue;
            }

            if sql_text::is_mysql_dash_comment_start(bytes, index) {
                in_line_comment = true;
                index += 2;
                continue;
            }

            if byte == b'#' {
                in_line_comment = true;
                index += 1;
                continue;
            }

            if byte == b'/' && next == Some(b'*') {
                in_block_comment = true;
                index += 2;
                continue;
            }

            match byte {
                b'\'' => in_single_quote = true,
                b'"' => in_double_quote = true,
                b'`' => in_backtick = true,
                b'(' => depth += 1,
                b')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(index);
                    }
                }
                _ => {}
            }

            index += 1;
        }

        None
    }

    fn split_top_level_comma_list(source: &str) -> Vec<String> {
        let bytes = source.as_bytes();
        let mut items = Vec::new();
        let mut start = 0usize;
        let mut index = 0usize;
        let mut depth = 0usize;
        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut in_backtick = false;
        let mut in_line_comment = false;
        let mut in_block_comment = false;

        while index < bytes.len() {
            let byte = bytes[index];
            let next = bytes.get(index + 1).copied();

            if in_line_comment {
                if byte == b'\n' {
                    in_line_comment = false;
                }
                index += 1;
                continue;
            }

            if in_block_comment {
                if byte == b'*' && next == Some(b'/') {
                    in_block_comment = false;
                    index += 2;
                } else {
                    index += 1;
                }
                continue;
            }

            if in_single_quote {
                if byte == b'\\' && next.is_some() {
                    index += 2;
                    continue;
                }
                if byte == b'\'' {
                    if next == Some(b'\'') {
                        index += 2;
                        continue;
                    }
                    in_single_quote = false;
                }
                index += 1;
                continue;
            }

            if in_double_quote {
                if byte == b'\\' && next.is_some() {
                    index += 2;
                    continue;
                }
                if byte == b'"' {
                    if next == Some(b'"') {
                        index += 2;
                        continue;
                    }
                    in_double_quote = false;
                }
                index += 1;
                continue;
            }

            if in_backtick {
                if byte == b'`' {
                    if next == Some(b'`') {
                        index += 2;
                        continue;
                    }
                    in_backtick = false;
                }
                index += 1;
                continue;
            }

            if sql_text::is_mysql_dash_comment_start(bytes, index) {
                in_line_comment = true;
                index += 2;
                continue;
            }

            if byte == b'#' {
                in_line_comment = true;
                index += 1;
                continue;
            }

            if byte == b'/' && next == Some(b'*') {
                in_block_comment = true;
                index += 2;
                continue;
            }

            match byte {
                b'\'' => in_single_quote = true,
                b'"' => in_double_quote = true,
                b'`' => in_backtick = true,
                b'(' => depth += 1,
                b')' => depth = depth.saturating_sub(1),
                b',' if depth == 0 => {
                    if let Some(item) = source.get(start..index) {
                        let trimmed = item.trim();
                        if !trimmed.is_empty() {
                            items.push(trimmed.to_string());
                        }
                    }
                    start = index + 1;
                }
                _ => {}
            }

            index += 1;
        }

        if let Some(item) = source.get(start..) {
            let trimmed = item.trim();
            if !trimmed.is_empty() {
                items.push(trimmed.to_string());
            }
        }

        items
    }

    fn parse_mysql_parameter(parameter: &str, position: i32) -> Option<ProcedureArgument> {
        let parameter = Self::strip_mysql_inline_comments(parameter);
        let parameter = parameter.trim();
        let mut index = Self::skip_ascii_whitespace(parameter, 0);

        let mut direction = "IN".to_string();

        for candidate in ["INOUT", "OUT", "IN"] {
            if Self::keyword_matches_at(parameter, index, candidate) {
                direction = candidate.to_string();
                index = Self::skip_ascii_whitespace(parameter, index + candidate.len());
                break;
            }
        }

        let name_end = Self::parse_identifier_segment_end(parameter, index)?;
        let name_raw = parameter.get(index..name_end)?;
        let name = Self::unquote_identifier(name_raw);

        let remainder = parameter.get(name_end..)?.trim();
        if remainder.is_empty() {
            return None;
        }

        let (data_type, default_value) =
            if let Some(default_idx) = Self::find_keyword_at_top_level(remainder, "DEFAULT", 0) {
                let type_part = remainder
                    .get(..default_idx)
                    .map(str::trim)
                    .unwrap_or_default()
                    .to_string();
                let default_part = remainder
                    .get(default_idx + "DEFAULT".len()..)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned);
                (type_part, default_part)
            } else {
                (remainder.to_string(), None)
            };

        if data_type.trim().is_empty() {
            return None;
        }

        Some(ProcedureArgument {
            name: Some(name),
            position,
            sequence: position,
            data_type: Some(data_type.trim().to_string()),
            in_out: Some(direction),
            data_length: None,
            data_precision: None,
            data_scale: None,
            type_owner: None,
            type_name: None,
            pls_type: None,
            overload: None,
            default_value,
        })
    }

    fn parse_function_return_type(ddl: &str, close_paren_index: usize) -> Option<String> {
        let returns_index = Self::find_keyword_at_top_level(ddl, "RETURNS", close_paren_index)?;
        let type_start = Self::skip_ascii_whitespace(ddl, returns_index + "RETURNS".len());
        let type_section = ddl.get(type_start..)?.trim();
        if type_section.is_empty() {
            return None;
        }

        let mut type_end = type_section.len();
        for keyword in [
            "DETERMINISTIC",
            "NOT",
            "CONTAINS",
            "NO",
            "READS",
            "MODIFIES",
            "SQL",
            "COMMENT",
            "BEGIN",
            "RETURN",
        ] {
            if let Some(position) = Self::find_keyword_at_top_level(type_section, keyword, 0) {
                type_end = type_end.min(position);
            }
        }

        type_section
            .get(..type_end)
            .map(Self::strip_mysql_inline_comments)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    fn parse_routine_arguments_from_create_ddl(
        ddl: &str,
        routine_type: &str,
    ) -> Option<Vec<ProcedureArgument>> {
        let routine_type = routine_type.trim();
        let keyword_index = Self::find_keyword_at_top_level(ddl, routine_type, 0)?;
        let name_start = Self::skip_ascii_whitespace(ddl, keyword_index + routine_type.len());
        let name_end = Self::parse_identifier_path_end(ddl, name_start)?;
        let open_paren_index = Self::skip_ascii_whitespace(ddl, name_end);
        if ddl.as_bytes().get(open_paren_index) != Some(&b'(') {
            return None;
        }

        let close_paren_index = Self::find_matching_paren(ddl, open_paren_index)?;
        let params_source = ddl.get(open_paren_index + 1..close_paren_index)?;
        let mut arguments = Vec::new();

        for (index, parameter) in Self::split_top_level_comma_list(params_source)
            .into_iter()
            .enumerate()
        {
            let position = i32::try_from(index + 1).ok().unwrap_or(i32::MAX);
            if let Some(argument) = Self::parse_mysql_parameter(&parameter, position) {
                arguments.push(argument);
            }
        }

        if routine_type.eq_ignore_ascii_case("FUNCTION") {
            if let Some(return_type) = Self::parse_function_return_type(ddl, close_paren_index) {
                arguments.insert(
                    0,
                    ProcedureArgument {
                        name: None,
                        position: 0,
                        sequence: 0,
                        data_type: Some(return_type),
                        in_out: Some("RETURN".to_string()),
                        data_length: None,
                        data_precision: None,
                        data_scale: None,
                        type_owner: None,
                        type_name: None,
                        pls_type: None,
                        overload: None,
                        default_value: None,
                    },
                );
            }
        }

        Some(arguments)
    }

    fn fallback_routine_arguments_from_ddl(
        conn: &mut Conn,
        schema_name: Option<&str>,
        routine_name: &str,
    ) -> Option<Vec<ProcedureArgument>> {
        let schema_name = Self::optional_schema_param(schema_name);
        let routine_type: Option<String> = conn
            .exec_first(
                "SELECT ROUTINE_TYPE \
                 FROM INFORMATION_SCHEMA.ROUTINES \
                 WHERE ROUTINE_SCHEMA = COALESCE(?, DATABASE()) AND ROUTINE_NAME = ? \
                 LIMIT 1",
                (schema_name.clone(), routine_name),
            )
            .ok()
            .flatten();
        let routine_type = routine_type?;
        let ddl = Self::get_create_object_in_schema(
            conn,
            schema_name.as_deref(),
            &routine_type,
            routine_name,
        )
        .ok()?;
        if ddl.trim().is_empty() {
            return None;
        }
        Self::parse_routine_arguments_from_create_ddl(&ddl, &routine_type)
    }

    pub fn get_tables(conn: &mut Conn) -> Result<Vec<String>, MysqlError> {
        let rows: Vec<String> = conn.query(
            "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE' \
             ORDER BY TABLE_NAME",
        )?;
        Ok(rows)
    }

    pub fn get_views(conn: &mut Conn) -> Result<Vec<String>, MysqlError> {
        let rows: Vec<String> = conn.query(
            "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.VIEWS \
             WHERE TABLE_SCHEMA = DATABASE() \
             ORDER BY TABLE_NAME",
        )?;
        Ok(rows)
    }

    pub fn get_procedures(conn: &mut Conn) -> Result<Vec<String>, MysqlError> {
        let rows: Vec<String> = conn.query(
            "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINES \
             WHERE ROUTINE_SCHEMA = DATABASE() AND ROUTINE_TYPE = 'PROCEDURE' \
             ORDER BY ROUTINE_NAME",
        )?;
        Ok(rows)
    }

    pub fn get_functions(conn: &mut Conn) -> Result<Vec<String>, MysqlError> {
        let rows: Vec<String> = conn.query(
            "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINES \
             WHERE ROUTINE_SCHEMA = DATABASE() AND ROUTINE_TYPE = 'FUNCTION' \
             ORDER BY ROUTINE_NAME",
        )?;
        Ok(rows)
    }

    pub fn get_schemas(conn: &mut Conn) -> Result<Vec<String>, MysqlError> {
        let rows: Vec<String> = conn.query(
            "SELECT SCHEMA_NAME FROM INFORMATION_SCHEMA.SCHEMATA \
             WHERE SCHEMA_NAME NOT IN ('information_schema', 'mysql', 'performance_schema') \
             ORDER BY SCHEMA_NAME",
        )?;
        Ok(rows)
    }

    pub fn get_schema_objects_by_schema(
        conn: &mut Conn,
    ) -> Result<HashMap<String, Vec<(String, String)>>, MysqlError> {
        let rows: Vec<(String, String, String)> = conn.query(
            "SELECT TABLE_SCHEMA, TABLE_NAME, \
                    CASE TABLE_TYPE \
                        WHEN 'BASE TABLE' THEN 'TABLE' \
                        ELSE TABLE_TYPE \
                    END AS OBJECT_TYPE \
             FROM INFORMATION_SCHEMA.TABLES \
             WHERE TABLE_SCHEMA NOT IN ('information_schema', 'mysql', 'performance_schema') \
               AND TABLE_TYPE IN ('BASE TABLE', 'VIEW', 'SEQUENCE') \
             UNION ALL \
             SELECT ROUTINE_SCHEMA, ROUTINE_NAME, ROUTINE_TYPE \
             FROM INFORMATION_SCHEMA.ROUTINES \
             WHERE ROUTINE_SCHEMA NOT IN ('information_schema', 'mysql', 'performance_schema') \
             UNION ALL \
             SELECT TRIGGER_SCHEMA, TRIGGER_NAME, 'TRIGGER' \
             FROM INFORMATION_SCHEMA.TRIGGERS \
             WHERE TRIGGER_SCHEMA NOT IN ('information_schema', 'mysql', 'performance_schema') \
             UNION ALL \
             SELECT EVENT_SCHEMA, EVENT_NAME, 'EVENT' \
             FROM INFORMATION_SCHEMA.EVENTS \
             WHERE EVENT_SCHEMA NOT IN ('information_schema', 'mysql', 'performance_schema') \
             ORDER BY 1, 2, 3",
        )?;

        let mut grouped = HashMap::new();
        for (schema, name, object_type) in rows {
            grouped
                .entry(schema)
                .or_insert_with(Vec::new)
                .push((name, object_type));
        }
        Ok(grouped)
    }

    pub fn get_schema_relation_members_by_schema(
        conn: &mut Conn,
    ) -> Result<HashMap<String, Vec<String>>, MysqlError> {
        let rows: Vec<(String, String)> = conn.query(
            "SELECT TABLE_SCHEMA, TABLE_NAME FROM INFORMATION_SCHEMA.TABLES \
             WHERE TABLE_SCHEMA NOT IN ('information_schema', 'mysql', 'performance_schema') \
               AND TABLE_TYPE IN ('BASE TABLE', 'VIEW') \
             ORDER BY 1, 2",
        )?;

        let mut grouped = HashMap::new();
        for (schema, name) in rows {
            grouped.entry(schema).or_insert_with(Vec::new).push(name);
        }
        Ok(grouped)
    }

    pub fn get_triggers(conn: &mut Conn) -> Result<Vec<String>, MysqlError> {
        let rows: Vec<String> = conn.query(
            "SELECT TRIGGER_NAME FROM INFORMATION_SCHEMA.TRIGGERS \
             WHERE TRIGGER_SCHEMA = DATABASE() \
             ORDER BY TRIGGER_NAME",
        )?;
        Ok(rows)
    }

    pub fn get_sequences(conn: &mut Conn) -> Result<Vec<String>, MysqlError> {
        let rows: Vec<String> = conn.query(
            "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'SEQUENCE' \
             ORDER BY TABLE_NAME",
        )?;
        Ok(rows)
    }

    pub fn get_events(conn: &mut Conn) -> Result<Vec<String>, MysqlError> {
        let rows: Vec<String> = conn.query(
            "SELECT EVENT_NAME FROM INFORMATION_SCHEMA.EVENTS \
             WHERE EVENT_SCHEMA = DATABASE() \
             ORDER BY EVENT_NAME",
        )?;
        Ok(rows)
    }

    pub fn get_indexes(conn: &mut Conn, table_name: &str) -> Result<Vec<String>, MysqlError> {
        Self::get_indexes_in_schema(conn, None, table_name)
    }

    pub fn get_indexes_in_schema(
        conn: &mut Conn,
        schema_name: Option<&str>,
        table_name: &str,
    ) -> Result<Vec<String>, MysqlError> {
        let schema_name = Self::optional_schema_param(schema_name);
        let rows: Vec<String> = conn.exec(
            "SELECT DISTINCT INDEX_NAME FROM INFORMATION_SCHEMA.STATISTICS \
             WHERE TABLE_SCHEMA = COALESCE(?, DATABASE()) AND TABLE_NAME = ? \
             ORDER BY INDEX_NAME",
            (schema_name, table_name),
        )?;
        Ok(rows)
    }

    pub fn get_index_details(
        conn: &mut Conn,
        table_name: &str,
    ) -> Result<Vec<IndexInfo>, MysqlError> {
        Self::get_index_details_in_schema(conn, None, table_name)
    }

    pub fn get_index_details_in_schema(
        conn: &mut Conn,
        schema_name: Option<&str>,
        table_name: &str,
    ) -> Result<Vec<IndexInfo>, MysqlError> {
        let schema_name = Self::optional_schema_param(schema_name);
        let rows: Vec<(String, u64, Option<String>)> = conn.exec(
            "SELECT INDEX_NAME, NON_UNIQUE, \
             GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ', ') \
             FROM INFORMATION_SCHEMA.STATISTICS \
             WHERE TABLE_SCHEMA = COALESCE(?, DATABASE()) AND TABLE_NAME = ? \
             GROUP BY INDEX_NAME, NON_UNIQUE \
             ORDER BY INDEX_NAME",
            (schema_name, table_name),
        )?;

        Ok(rows
            .into_iter()
            .map(|(name, non_unique, columns)| IndexInfo {
                name,
                is_unique: non_unique == 0,
                columns: columns.unwrap_or_default(),
            })
            .collect())
    }

    pub fn describe_object(
        conn: &mut Conn,
        table_name: &str,
    ) -> Result<Vec<MysqlTableColumnDetail>, MysqlError> {
        let rows: Vec<(String, String, String, Option<String>, String, String)> = conn.exec(
            "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_KEY, EXTRA \
             FROM INFORMATION_SCHEMA.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? \
             ORDER BY ORDINAL_POSITION",
            (table_name,),
        )?;

        Ok(rows
            .into_iter()
            .map(
                |(name, data_type, nullable, default_value, column_key, extra)| {
                    MysqlTableColumnDetail {
                        name,
                        data_type,
                        is_nullable: nullable == "YES",
                        default_value,
                        is_primary_key: column_key == "PRI",
                        extra,
                    }
                },
            )
            .collect())
    }

    pub fn get_table_structure(
        conn: &mut Conn,
        table_name: &str,
    ) -> Result<Vec<TableColumnDetail>, MysqlError> {
        Self::get_table_structure_in_schema(conn, None, table_name)
    }

    pub fn get_table_structure_in_schema(
        conn: &mut Conn,
        schema_name: Option<&str>,
        table_name: &str,
    ) -> Result<Vec<TableColumnDetail>, MysqlError> {
        let schema_name = Self::optional_schema_param(schema_name);
        let rows: Vec<(
            String,
            String,
            Option<u64>,
            Option<u64>,
            Option<u64>,
            String,
            Option<String>,
            String,
        )> = conn.exec(
            "SELECT COLUMN_NAME, COLUMN_TYPE, CHARACTER_MAXIMUM_LENGTH, \
             NUMERIC_PRECISION, NUMERIC_SCALE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_KEY \
             FROM INFORMATION_SCHEMA.COLUMNS \
             WHERE TABLE_SCHEMA = COALESCE(?, DATABASE()) AND TABLE_NAME = ? \
             ORDER BY ORDINAL_POSITION",
            (schema_name, table_name),
        )?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    name,
                    data_type,
                    data_length,
                    data_precision,
                    data_scale,
                    nullable,
                    default_value,
                    column_key,
                )| TableColumnDetail {
                    name,
                    data_type,
                    data_length: data_length
                        .and_then(|value| i32::try_from(value).ok())
                        .unwrap_or(0),
                    data_precision: data_precision.and_then(|value| i32::try_from(value).ok()),
                    data_scale: data_scale.and_then(|value| i32::try_from(value).ok()),
                    nullable: nullable == "YES",
                    default_value,
                    is_primary_key: column_key == "PRI",
                },
            )
            .collect())
    }

    pub fn get_table_columns(
        conn: &mut Conn,
        table_name: &str,
    ) -> Result<Vec<ColumnInfo>, MysqlError> {
        Self::get_table_columns_in_schema(conn, None, table_name)
    }

    pub fn get_table_columns_in_schema(
        conn: &mut Conn,
        schema_name: Option<&str>,
        table_name: &str,
    ) -> Result<Vec<ColumnInfo>, MysqlError> {
        let schema_name = Self::optional_schema_param(schema_name);
        let rows: Vec<(String, String)> = conn.exec(
            "SELECT COLUMN_NAME, COLUMN_TYPE \
             FROM INFORMATION_SCHEMA.COLUMNS \
             WHERE TABLE_SCHEMA = COALESCE(?, DATABASE()) AND TABLE_NAME = ? \
             ORDER BY ORDINAL_POSITION",
            (schema_name, table_name),
        )?;

        Ok(rows
            .into_iter()
            .map(|(name, data_type)| ColumnInfo {
                name,
                data_type,
                kind: SqlValueKind::Unknown,
            })
            .collect())
    }

    pub fn get_databases(conn: &mut Conn) -> Result<Vec<String>, MysqlError> {
        let rows: Vec<String> =
            conn.query("SELECT SCHEMA_NAME FROM INFORMATION_SCHEMA.SCHEMATA ORDER BY SCHEMA_NAME")?;
        Ok(rows)
    }

    pub fn get_create_table(conn: &mut Conn, table_name: &str) -> Result<String, MysqlError> {
        let result: Option<(String, String)> = conn.exec_first(
            format!("SHOW CREATE TABLE {}", Self::quoted_identifier(table_name)),
            (),
        )?;
        match result {
            Some((_, ddl)) => Ok(ddl),
            None => Ok(String::new()),
        }
    }

    pub fn get_table_constraints(
        conn: &mut Conn,
        table_name: &str,
    ) -> Result<Vec<ConstraintInfo>, MysqlError> {
        Self::get_table_constraints_in_schema(conn, None, table_name)
    }

    pub fn get_table_constraints_in_schema(
        conn: &mut Conn,
        schema_name: Option<&str>,
        table_name: &str,
    ) -> Result<Vec<ConstraintInfo>, MysqlError> {
        let schema_name = Self::optional_schema_param(schema_name);
        let rows: Vec<(String, String, Option<String>, Option<String>)> = conn.exec(
            "SELECT tc.CONSTRAINT_NAME, tc.CONSTRAINT_TYPE, \
             GROUP_CONCAT(kcu.COLUMN_NAME ORDER BY kcu.ORDINAL_POSITION SEPARATOR ', ') AS columns, \
             rc.REFERENCED_TABLE_NAME \
             FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc \
             LEFT JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu \
               ON tc.CONSTRAINT_SCHEMA = kcu.CONSTRAINT_SCHEMA \
              AND tc.TABLE_NAME = kcu.TABLE_NAME \
              AND tc.CONSTRAINT_NAME = kcu.CONSTRAINT_NAME \
             LEFT JOIN INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS rc \
               ON tc.CONSTRAINT_SCHEMA = rc.CONSTRAINT_SCHEMA \
              AND tc.TABLE_NAME = rc.TABLE_NAME \
              AND tc.CONSTRAINT_NAME = rc.CONSTRAINT_NAME \
             WHERE tc.TABLE_SCHEMA = COALESCE(?, DATABASE()) AND tc.TABLE_NAME = ? \
             GROUP BY tc.CONSTRAINT_NAME, tc.CONSTRAINT_TYPE, rc.REFERENCED_TABLE_NAME \
             ORDER BY tc.CONSTRAINT_TYPE, tc.CONSTRAINT_NAME",
            (schema_name, table_name),
        )?;

        Ok(rows
            .into_iter()
            .map(
                |(name, constraint_type, columns, ref_table)| ConstraintInfo {
                    name,
                    constraint_type,
                    columns: columns.unwrap_or_default(),
                    ref_table,
                },
            )
            .collect())
    }

    pub fn get_table_foreign_keys(
        conn: &mut Conn,
        table_name: &str,
    ) -> Result<Vec<ForeignKeyInfo>, MysqlError> {
        Self::get_table_foreign_keys_in_schema(conn, None, table_name)
    }

    pub fn get_table_foreign_keys_in_schema(
        conn: &mut Conn,
        schema_name: Option<&str>,
        table_name: &str,
    ) -> Result<Vec<ForeignKeyInfo>, MysqlError> {
        let schema_name = Self::optional_schema_param(schema_name);
        let rows: Vec<(String, String, String, String)> = conn.exec(
            "SELECT kcu.CONSTRAINT_NAME, kcu.COLUMN_NAME, \
             kcu.REFERENCED_TABLE_NAME, kcu.REFERENCED_COLUMN_NAME \
             FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu \
             WHERE kcu.TABLE_SCHEMA = COALESCE(?, DATABASE()) AND kcu.TABLE_NAME = ? \
               AND kcu.REFERENCED_TABLE_NAME IS NOT NULL \
             ORDER BY kcu.CONSTRAINT_NAME, kcu.ORDINAL_POSITION",
            (schema_name, table_name),
        )?;

        let mut grouped: Vec<(String, ForeignKeyInfo)> = Vec::new();
        for (constraint_name, local_col, ref_table, ref_col) in rows {
            match grouped.last_mut() {
                Some((name, fk)) if *name == constraint_name => {
                    fk.columns.push(local_col);
                    fk.ref_columns.push(ref_col);
                }
                _ => grouped.push((
                    constraint_name,
                    ForeignKeyInfo {
                        columns: vec![local_col],
                        ref_table,
                        ref_columns: vec![ref_col],
                    },
                )),
            }
        }
        Ok(grouped.into_iter().map(|(_, fk)| fk).collect())
    }

    pub fn get_create_object(
        conn: &mut Conn,
        object_type: &str,
        object_name: &str,
    ) -> Result<String, MysqlError> {
        Self::get_create_object_in_schema(conn, None, object_type, object_name)
    }

    pub fn get_create_object_in_schema(
        conn: &mut Conn,
        schema_name: Option<&str>,
        object_type: &str,
        object_name: &str,
    ) -> Result<String, MysqlError> {
        let object_type_upper = object_type.to_ascii_uppercase();
        if Self::create_ddl_column_names(&object_type_upper).is_empty() {
            return Ok(String::new());
        }

        let qualified_name = if let Some(schema) = Self::optional_schema_param(schema_name) {
            format!(
                "{}.{}",
                Self::quoted_identifier(&schema),
                Self::quoted_identifier(object_name)
            )
        } else {
            Self::quoted_identifier(object_name)
        };

        let sql = format!("SHOW CREATE {} {}", object_type_upper, qualified_name);
        let mut result = conn.query_iter(sql)?;
        let ddl_column_index = result.columns().as_ref().iter().position(|column| {
            let column_name = column.name_str();
            Self::create_ddl_column_names(&object_type_upper)
                .iter()
                .any(|candidate| column_name.eq_ignore_ascii_case(candidate))
        });
        let Some(row_result) = result.next() else {
            return Ok(String::new());
        };
        let row = row_result?;
        let ddl = ddl_column_index
            .and_then(|index| row.as_ref(index))
            .map(MysqlExecutor::value_to_string)
            .unwrap_or_default();
        Ok(ddl)
    }

    pub fn get_object_types(conn: &mut Conn, object_name: &str) -> Result<Vec<String>, MysqlError> {
        Self::get_object_types_in_schema(conn, None, object_name)
    }

    pub fn get_object_types_in_schema(
        conn: &mut Conn,
        schema_name: Option<&str>,
        object_name: &str,
    ) -> Result<Vec<String>, MysqlError> {
        let schema_name = Self::optional_schema_param(schema_name);

        let mut object_types: Vec<String> = conn.exec(
            "SELECT CASE WHEN TABLE_TYPE = 'VIEW' THEN 'VIEW' ELSE 'TABLE' END \
             FROM INFORMATION_SCHEMA.TABLES \
             WHERE TABLE_SCHEMA = COALESCE(?, DATABASE()) AND TABLE_NAME = ?",
            (schema_name.clone(), object_name),
        )?;

        let mut routine_types: Vec<String> = conn.exec(
            "SELECT ROUTINE_TYPE \
             FROM INFORMATION_SCHEMA.ROUTINES \
             WHERE ROUTINE_SCHEMA = COALESCE(?, DATABASE()) AND ROUTINE_NAME = ?",
            (schema_name.clone(), object_name),
        )?;
        object_types.append(&mut routine_types);

        let mut trigger_types: Vec<String> = conn.exec(
            "SELECT 'TRIGGER' \
             FROM INFORMATION_SCHEMA.TRIGGERS \
             WHERE TRIGGER_SCHEMA = COALESCE(?, DATABASE()) AND TRIGGER_NAME = ?",
            (schema_name.clone(), object_name),
        )?;
        object_types.append(&mut trigger_types);

        let mut sequence_types: Vec<String> = conn.exec(
            "SELECT 'SEQUENCE' \
             FROM INFORMATION_SCHEMA.TABLES \
             WHERE TABLE_SCHEMA = COALESCE(?, DATABASE()) AND TABLE_NAME = ? \
               AND TABLE_TYPE = 'SEQUENCE'",
            (schema_name.clone(), object_name),
        )?;
        object_types.append(&mut sequence_types);

        let mut event_types: Vec<String> = conn.exec(
            "SELECT 'EVENT' \
             FROM INFORMATION_SCHEMA.EVENTS \
             WHERE EVENT_SCHEMA = COALESCE(?, DATABASE()) AND EVENT_NAME = ?",
            (schema_name, object_name),
        )?;
        object_types.append(&mut event_types);

        object_types.sort();
        object_types.dedup();
        Ok(object_types)
    }

    pub fn get_routine_arguments(
        conn: &mut Conn,
        routine_name: &str,
    ) -> Result<Vec<ProcedureArgument>, MysqlError> {
        Self::get_routine_arguments_in_schema(conn, None, routine_name)
    }

    pub fn get_routine_arguments_in_schema(
        conn: &mut Conn,
        schema_name: Option<&str>,
        routine_name: &str,
    ) -> Result<Vec<ProcedureArgument>, MysqlError> {
        let schema_name = Self::optional_schema_param(schema_name);
        let query_schema_name = schema_name.clone();
        let rows_result: Result<
            Vec<(
                Option<String>,
                u64,
                Option<String>,
                Option<String>,
                Option<u64>,
                Option<u64>,
                Option<u64>,
            )>,
            MysqlError,
        > = conn.exec(
            "SELECT PARAMETER_NAME, ORDINAL_POSITION, PARAMETER_MODE, DTD_IDENTIFIER, \
             CHARACTER_MAXIMUM_LENGTH, NUMERIC_PRECISION, NUMERIC_SCALE \
             FROM INFORMATION_SCHEMA.PARAMETERS \
             WHERE SPECIFIC_SCHEMA = COALESCE(?, DATABASE()) AND SPECIFIC_NAME = ? \
             ORDER BY ORDINAL_POSITION",
            (query_schema_name, routine_name),
        );

        let fallback_arguments = |conn: &mut Conn| {
            Self::fallback_routine_arguments_from_ddl(conn, schema_name.as_deref(), routine_name)
        };

        let rows = match rows_result {
            Ok(rows) if !rows.is_empty() => rows,
            Ok(_) => {
                return Ok(fallback_arguments(conn).unwrap_or_default());
            }
            Err(err) => {
                if let Some(arguments) = fallback_arguments(conn) {
                    return Ok(arguments);
                }
                return Err(err);
            }
        };

        Ok(rows
            .into_iter()
            .map(
                |(
                    name,
                    position,
                    parameter_mode,
                    data_type,
                    data_length,
                    data_precision,
                    data_scale,
                )| {
                    let in_out = if position == 0 && name.is_none() {
                        Some("RETURN".to_string())
                    } else {
                        parameter_mode
                    };

                    ProcedureArgument {
                        name,
                        position: i32::try_from(position).ok().unwrap_or(i32::MAX),
                        sequence: i32::try_from(position).ok().unwrap_or(i32::MAX),
                        data_type,
                        in_out,
                        data_length: data_length.and_then(|value| i32::try_from(value).ok()),
                        data_precision: data_precision.and_then(|value| i32::try_from(value).ok()),
                        data_scale: data_scale.and_then(|value| i32::try_from(value).ok()),
                        type_owner: None,
                        type_name: None,
                        pls_type: None,
                        overload: None,
                        default_value: None,
                    }
                },
            )
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MysqlExecutor, MysqlObjectBrowser, MysqlResultSetSnapshot, MysqlSessionTimeoutRestore,
    };
    use crate::db::connection::{ConnectionInfo, DatabaseType};
    use crate::db::query::types::ColumnInfo;
    use crate::db::DatabaseConnection;
    use mysql::prelude::Queryable;
    use mysql::{Error as MysqlError, MySqlError, Value as MysqlValue};
    use std::env;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct RecordingQueryable {
        statements: Vec<String>,
        fail_on_query_drop_containing: Option<String>,
    }

    impl Queryable for RecordingQueryable {
        fn query_iter<Q: AsRef<str>>(
            &mut self,
            _query: Q,
        ) -> mysql::Result<mysql::QueryResult<'_, '_, '_, mysql::Text>> {
            unreachable!("RecordingQueryable tests override query_drop directly")
        }

        fn query_drop<Q: AsRef<str>>(&mut self, query: Q) -> mysql::Result<()> {
            let statement = query.as_ref().to_string();
            self.statements.push(statement.clone());
            if self
                .fail_on_query_drop_containing
                .as_deref()
                .is_some_and(|needle| statement.contains(needle))
            {
                return Err(MysqlError::MySqlError(MySqlError {
                    state: "HY000".to_string(),
                    code: 1234,
                    message: format!("forced failure for {statement}"),
                }));
            }
            Ok(())
        }

        fn prep<Q: AsRef<str>>(&mut self, _query: Q) -> mysql::Result<mysql::Statement> {
            unreachable!("RecordingQueryable does not prepare statements")
        }

        fn close(&mut self, _stmt: mysql::Statement) -> mysql::Result<()> {
            unreachable!("RecordingQueryable does not prepare statements")
        }

        fn exec_iter<S, P>(
            &mut self,
            _stmt: S,
            _params: P,
        ) -> mysql::Result<mysql::QueryResult<'_, '_, '_, mysql::Binary>>
        where
            S: mysql::prelude::AsStatement,
            P: Into<mysql::Params>,
        {
            unreachable!("RecordingQueryable does not execute prepared statements")
        }
    }

    #[test]
    fn cancelled_select_result_is_terminal_cancel_with_partial_rows() {
        let columns = vec![ColumnInfo {
            name: "id".to_string(),
            data_type: "Long".to_string(),
            kind: crate::db::SqlValueKind::Unknown,
        }];
        let rows = vec![vec!["1".to_string()]];

        let result = MysqlExecutor::cancelled_select_result(
            "select id from t",
            columns.clone(),
            rows.clone(),
            Duration::from_millis(7),
        );

        assert!(!result.success);
        assert!(result.is_select);
        assert_eq!(result.message, "Query cancelled");
        assert_eq!(result.columns.len(), columns.len());
        assert_eq!(result.columns[0].name, columns[0].name);
        assert_eq!(result.columns[0].data_type, columns[0].data_type);
        assert_eq!(result.rows, rows);
        assert_eq!(result.row_count, 1);
    }

    #[test]
    fn mysql_value_to_string_formats_common_non_text_types() {
        assert_eq!(MysqlExecutor::value_to_string(&MysqlValue::Int(-7)), "-7");
        assert_eq!(MysqlExecutor::value_to_string(&MysqlValue::UInt(42)), "42");
        assert_eq!(
            MysqlExecutor::value_to_string(&MysqlValue::Date(2026, 4, 5, 13, 14, 15, 123_456)),
            "2026-04-05 13:14:15.123456"
        );
        assert_eq!(
            MysqlExecutor::value_to_string(&MysqlValue::Time(true, 1, 2, 3, 4, 0)),
            "-26:03:04"
        );
    }

    #[test]
    fn mysql_escape_identifier_doubles_backticks() {
        assert_eq!(
            super::MysqlObjectBrowser::quoted_identifier("odd`name"),
            "`odd``name`"
        );
    }

    #[test]
    fn transaction_control_sql_preserves_non_plain_variants() {
        assert_eq!(
            MysqlExecutor::transaction_control_sql_to_execute("COMMIT", "COMMIT"),
            "COMMIT"
        );
        assert_eq!(
            MysqlExecutor::transaction_control_sql_to_execute("COMMIT AND CHAIN", "COMMIT"),
            "COMMIT AND CHAIN"
        );
        assert_eq!(
            MysqlExecutor::transaction_control_sql_to_execute(
                "COMMIT -- keep chaining\nAND CHAIN",
                "COMMIT"
            ),
            "COMMIT -- keep chaining\nAND CHAIN"
        );
        assert_eq!(
            MysqlExecutor::transaction_control_sql_to_execute(
                "ROLLBACK TO SAVEPOINT sp1",
                "ROLLBACK"
            ),
            "ROLLBACK TO SAVEPOINT sp1"
        );
        assert_eq!(
            MysqlExecutor::transaction_control_sql_to_execute(
                "ROLLBACK -- keep chaining\nAND CHAIN",
                "ROLLBACK"
            ),
            "ROLLBACK -- keep chaining\nAND CHAIN"
        );
    }

    #[test]
    fn mysql_build_explain_sql_trims_statement_terminator() {
        assert_eq!(
            MysqlExecutor::build_explain_sql("  SELECT * FROM employees;   "),
            "EXPLAIN SELECT * FROM employees"
        );
    }

    #[test]
    fn mysql_build_explain_sql_keeps_existing_explain_statement() {
        assert_eq!(
            MysqlExecutor::build_explain_sql(" EXPLAIN SELECT * FROM employees; "),
            "EXPLAIN SELECT * FROM employees"
        );
        assert_eq!(
            MysqlExecutor::build_explain_sql("DESC employees;"),
            "DESC employees"
        );
    }

    #[test]
    fn mysql_classify_statement_treats_values_and_table_as_selects() {
        assert_eq!(
            MysqlExecutor::classify_statement("VALUES ROW(1, 'A')"),
            super::MysqlStatementKind::Select
        );
        assert_eq!(
            MysqlExecutor::classify_statement("TABLE employees"),
            super::MysqlStatementKind::Select
        );
    }

    #[test]
    fn mysql_executor_and_central_classifier_agree_on_read_only_result_statements() {
        for sql in ["VALUES ROW(1, 'A')", "TABLE employees"] {
            assert_eq!(
                crate::db::session_policy::classify_sql_for_db_type(DatabaseType::MySQL, sql),
                crate::db::session_policy::SqlKind::SelectLike,
                "{sql}"
            );
            assert_eq!(
                MysqlExecutor::classify_statement(sql),
                super::MysqlStatementKind::Select,
                "{sql}"
            );
        }
    }

    #[test]
    fn mysql_table_maintenance_display_routing_is_separate_from_session_safety() {
        for sql in [
            "ANALYZE TABLE t",
            "CHECK TABLE t",
            "CHECKSUM TABLE t",
            "OPTIMIZE TABLE t",
            "REPAIR TABLE t",
        ] {
            assert_eq!(
                crate::db::session_policy::classify_sql_for_db_type(DatabaseType::MySQL, sql),
                crate::db::session_policy::SqlKind::Ddl,
                "{sql} must not be SelectLike for cancel/timeout reuse"
            );
            assert_eq!(
                MysqlExecutor::classify_statement(sql),
                super::MysqlStatementKind::Select,
                "{sql} still returns a result set that should be displayed"
            );
        }
    }

    #[test]
    fn mysql_dml_returning_snapshot_becomes_displayable_result() {
        let result = MysqlExecutor::dml_result_from_snapshots(
            "INSERT INTO t(id) VALUES (1) RETURNING id",
            vec![MysqlResultSetSnapshot {
                columns: vec![ColumnInfo {
                    name: "id".to_string(),
                    data_type: "Long".to_string(),
                    kind: crate::db::SqlValueKind::Unknown,
                }],
                rows: vec![vec!["1".to_string()]],
                affected_rows: 1,
                info: String::new(),
            }],
            1,
            Duration::from_millis(1),
            "INSERT",
        );

        assert!(result.is_select);
        assert_eq!(result.columns[0].name, "id");
        assert_eq!(result.rows, vec![vec!["1".to_string()]]);
        assert_eq!(result.row_count, 1);
        assert_eq!(
            result.message,
            "INSERT 1 row(s) affected, 1 row(s) returned"
        );
    }

    #[test]
    fn mysql_dml_snapshot_fallback_preserves_affected_rows_for_plain_dml() {
        let result = MysqlExecutor::dml_result_from_snapshots(
            "UPDATE t SET name = 'x' WHERE id = 1",
            vec![],
            1,
            Duration::from_millis(1),
            "UPDATE",
        );

        assert!(!result.is_select);
        assert_eq!(result.row_count, 1);
        assert_eq!(result.message, "UPDATE 1 row(s) affected");
    }

    #[test]
    fn mysql_dml_returning_drained_snapshots_keep_first_result_shape() {
        let result = MysqlExecutor::dml_result_from_snapshots(
            "UPDATE t SET name = 'x' WHERE id = 1 RETURNING id, name",
            vec![
                MysqlResultSetSnapshot {
                    columns: vec![],
                    rows: vec![],
                    affected_rows: 1,
                    info: String::new(),
                },
                MysqlResultSetSnapshot {
                    columns: vec![
                        ColumnInfo {
                            name: "id".to_string(),
                            data_type: "Long".to_string(),
                            kind: crate::db::SqlValueKind::Unknown,
                        },
                        ColumnInfo {
                            name: "name".to_string(),
                            data_type: "VarString".to_string(),
                            kind: crate::db::SqlValueKind::Unknown,
                        },
                    ],
                    rows: vec![vec!["1".to_string(), "x".to_string()]],
                    affected_rows: 0,
                    info: String::new(),
                },
            ],
            1,
            Duration::from_millis(1),
            "UPDATE",
        );

        assert!(result.is_select);
        assert_eq!(
            result
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            vec!["id", "name"]
        );
        assert_eq!(result.rows, vec![vec!["1".to_string(), "x".to_string()]]);
        assert_eq!(
            result.message,
            "UPDATE 1 row(s) affected, 1 row(s) returned"
        );
    }

    #[test]
    fn mysql_classify_statement_treats_with_dml_as_non_select() {
        assert_eq!(
            MysqlExecutor::classify_statement(
                "WITH recent AS (SELECT 1 AS id) INSERT INTO audit_log(id) SELECT id FROM recent"
            ),
            super::MysqlStatementKind::Dml
        );
        assert_eq!(
            MysqlExecutor::classify_statement(
                "WITH recent AS (SELECT 1 AS id) UPDATE audit_log SET id = (SELECT id FROM recent)"
            ),
            super::MysqlStatementKind::Dml
        );
    }

    #[test]
    fn mysql_timeout_statement_uses_millisecond_session_setting() {
        assert_eq!(
            MysqlExecutor::mysql_timeout_statement(Some(Duration::from_secs(5))),
            "SET SESSION MAX_EXECUTION_TIME = 5000"
        );
        assert_eq!(
            MysqlExecutor::mysql_timeout_statement(None),
            "SET SESSION MAX_EXECUTION_TIME = 0"
        );
    }

    #[test]
    fn mysql_disabled_timeout_does_not_modify_the_session() {
        let mut conn = RecordingQueryable::default();

        let restore = MysqlExecutor::apply_session_timeout_with_restore_for_db(
            &mut conn,
            None,
            DatabaseType::MySQL,
        )
        .expect("disabled timeout should not require session SQL");

        assert!(restore.is_none());
        assert!(conn.statements.is_empty());
    }

    #[test]
    fn mariadb_statement_timeout_preserves_found_rows_without_session_sql() {
        let sql = "SELECT SQL_CALC_FOUND_ROWS id FROM t LIMIT 2";
        let timeout = Some(Duration::from_secs(60));

        assert_eq!(
            MysqlExecutor::statement_sql_preserving_found_rows_for_db_type(
                DatabaseType::MariaDB,
                sql,
                timeout,
                true,
            ),
            "SET STATEMENT max_statement_time = 60.000 FOR \
             SELECT SQL_CALC_FOUND_ROWS id FROM t LIMIT 2"
        );
        assert_eq!(
            MysqlExecutor::statement_sql_preserving_found_rows_for_db_type(
                DatabaseType::MySQL,
                sql,
                timeout,
                true,
            ),
            sql
        );
        assert_eq!(
            MysqlExecutor::statement_sql_preserving_found_rows_for_db_type(
                DatabaseType::MariaDB,
                "SELECT id FROM t",
                timeout,
                false,
            ),
            "SELECT id FROM t"
        );
    }

    #[test]
    fn mysql_timeout_provider_keeps_mariadb_query_timeout_separate() {
        assert_eq!(
            super::mysql_timeout_provider_for(DatabaseType::MySQL)
                .expect("MySQL timeout provider")
                .query_timeout_statement(Some(Duration::from_secs(5))),
            "SET SESSION MAX_EXECUTION_TIME = 5000"
        );
        assert_eq!(
            super::mysql_timeout_provider_for(DatabaseType::MariaDB)
                .expect("MariaDB timeout provider")
                .query_timeout_statement(Some(Duration::from_secs(5))),
            "SET SESSION max_statement_time = 5.000"
        );
        assert!(super::mysql_timeout_provider_for(DatabaseType::Oracle).is_err());
    }

    #[test]
    fn mysql_lock_wait_timeout_statements_follow_query_timeout_seconds() {
        assert_eq!(
            MysqlExecutor::lock_wait_timeout_statement(Duration::from_millis(500)),
            "SET SESSION lock_wait_timeout = 1"
        );
        assert_eq!(
            MysqlExecutor::innodb_lock_wait_timeout_statement(Duration::from_secs(5)),
            "SET SESSION innodb_lock_wait_timeout = 5"
        );
    }

    #[test]
    fn mysql_user_timeout_variable_sets_skip_app_timeout_wrapper() {
        let query_timeout = Some(Duration::from_secs(5));

        for sql in [
            "SET SESSION lock_wait_timeout = 120",
            "SET SESSION innodb_lock_wait_timeout = 120",
            "SET SESSION MAX_EXECUTION_TIME = 10000",
            "SET SESSION max_statement_time = 10",
            "SET @@session.MAX_EXECUTION_TIME = 10000",
            "SET STATEMENT max_statement_time = 10 FOR SELECT 1",
        ] {
            assert_eq!(
                crate::db::query::query_timeout_for_statement_for_db_type(
                    DatabaseType::MySQL,
                    sql,
                    query_timeout
                ),
                None,
                "{sql}"
            );
        }

        for sql in [
            "SELECT 1",
            "SET @note = 'lock_wait_timeout'",
            "SET @max_execution_time = 1",
            "SET @lock_wait_timeout = 5",
            "SET SESSION sql_notes = @@session.max_execution_time",
            "SELECT 'SET SESSION MAX_EXECUTION_TIME = 10000'",
        ] {
            assert_eq!(
                crate::db::query::query_timeout_for_statement_for_db_type(
                    DatabaseType::MySQL,
                    sql,
                    query_timeout
                ),
                query_timeout,
                "{sql}"
            );
        }
    }

    #[test]
    fn mysql_session_timeout_restore_uses_original_lock_wait_values() {
        let restore = MysqlSessionTimeoutRestore {
            max_execution_time: Some(5000),
            max_statement_time: None,
            lock_wait_timeout: Some(17),
            innodb_lock_wait_timeout: Some(123),
        };

        assert_eq!(
            restore.max_execution_time_statement(),
            Some("SET SESSION MAX_EXECUTION_TIME = 5000".to_string())
        );
        assert_eq!(
            restore.lock_wait_timeout_statement(),
            Some("SET SESSION lock_wait_timeout = 17".to_string())
        );
        assert_eq!(
            restore.innodb_lock_wait_timeout_statement(),
            Some("SET SESSION innodb_lock_wait_timeout = 123".to_string())
        );
    }

    #[test]
    fn mysql_session_timeout_restore_allows_partial_capture() {
        let restore = MysqlSessionTimeoutRestore {
            max_execution_time: None,
            max_statement_time: None,
            lock_wait_timeout: Some(17),
            innodb_lock_wait_timeout: None,
        };
        let mut conn = RecordingQueryable::default();

        restore
            .restore_for_db(&mut conn, DatabaseType::MySQL)
            .expect("partial timeout restore should restore captured variables");

        assert_eq!(
            conn.statements,
            vec!["SET SESSION lock_wait_timeout = 17".to_string()]
        );
    }

    #[test]
    fn mysql_session_timeout_restore_preserves_query_timeout_variable() {
        let restore = MysqlSessionTimeoutRestore {
            max_execution_time: Some(5000),
            max_statement_time: None,
            lock_wait_timeout: Some(17),
            innodb_lock_wait_timeout: None,
        };
        let mut conn = RecordingQueryable::default();

        restore
            .restore_for_db(&mut conn, DatabaseType::MySQL)
            .expect("MySQL timeout restore should restore captured query timeout");

        assert_eq!(
            conn.statements,
            vec![
                "SET SESSION MAX_EXECUTION_TIME = 5000".to_string(),
                "SET SESSION lock_wait_timeout = 17".to_string(),
            ]
        );
    }

    #[test]
    fn mariadb_session_timeout_restore_preserves_query_timeout_variable() {
        let restore = MysqlSessionTimeoutRestore {
            max_execution_time: None,
            max_statement_time: Some("2.500".to_string()),
            lock_wait_timeout: None,
            innodb_lock_wait_timeout: None,
        };
        let mut conn = RecordingQueryable::default();

        restore
            .restore_for_db(&mut conn, DatabaseType::MariaDB)
            .expect("MariaDB timeout restore should restore captured query timeout");

        assert_eq!(
            conn.statements,
            vec!["SET SESSION max_statement_time = 2.500".to_string()]
        );
    }

    #[test]
    fn mysql_family_timeout_restore_uses_provider_query_timeout_preference() {
        let restore = MysqlSessionTimeoutRestore {
            max_execution_time: Some(5000),
            max_statement_time: Some("2.500".to_string()),
            lock_wait_timeout: None,
            innodb_lock_wait_timeout: None,
        };
        let mut mysql_conn = RecordingQueryable::default();
        let mut mariadb_conn = RecordingQueryable::default();

        restore
            .restore_for_db(&mut mysql_conn, DatabaseType::MySQL)
            .expect("MySQL should prefer MAX_EXECUTION_TIME restore");
        restore
            .restore_for_db(&mut mariadb_conn, DatabaseType::MariaDB)
            .expect("MariaDB should prefer max_statement_time restore");

        assert_eq!(
            mysql_conn.statements,
            vec!["SET SESSION MAX_EXECUTION_TIME = 5000".to_string()]
        );
        assert_eq!(
            mariadb_conn.statements,
            vec!["SET SESSION max_statement_time = 2.500".to_string()]
        );
    }

    #[test]
    fn mysql_session_timeout_restore_reports_partial_restore_failure() {
        let restore = MysqlSessionTimeoutRestore {
            max_execution_time: Some(5000),
            max_statement_time: None,
            lock_wait_timeout: Some(17),
            innodb_lock_wait_timeout: Some(123),
        };
        let mut conn = RecordingQueryable {
            fail_on_query_drop_containing: Some("lock_wait_timeout".to_string()),
            ..RecordingQueryable::default()
        };

        let err = restore
            .restore_for_db(&mut conn, DatabaseType::MySQL)
            .expect_err("partial timeout restore must be reported to the caller");

        assert!(err.to_string().contains("forced failure"));
        assert_eq!(
            conn.statements,
            vec![
                "SET SESSION MAX_EXECUTION_TIME = 5000".to_string(),
                "SET SESSION lock_wait_timeout = 17".to_string(),
            ],
            "restore stops at the first failed variable, leaving the session unsafe to reuse"
        );
    }

    #[test]
    fn mysql_timeout_apply_failure_preserves_restore_failure_signal() {
        let restore = MysqlSessionTimeoutRestore {
            max_execution_time: Some(5000),
            max_statement_time: None,
            lock_wait_timeout: Some(17),
            innodb_lock_wait_timeout: None,
        };
        let mut conn = RecordingQueryable {
            fail_on_query_drop_containing: Some("lock_wait_timeout".to_string()),
            ..RecordingQueryable::default()
        };

        let err = MysqlExecutor::apply_session_timeout_with_captured_restore_for_db(
            &mut conn,
            Some(Duration::from_secs(5)),
            DatabaseType::MySQL,
            Some(restore),
        )
        .expect_err("apply failure followed by restore failure must be distinguishable");

        assert!(err.restore_failed());
        assert!(err.apply_error().to_string().contains("forced failure"));
        assert!(err
            .restore_error()
            .expect("restore error should be retained")
            .to_string()
            .contains("forced failure"));
        assert_eq!(
            conn.statements,
            vec![
                "SET SESSION MAX_EXECUTION_TIME = 5000".to_string(),
                "SET SESSION lock_wait_timeout = 5".to_string(),
                "SET SESSION MAX_EXECUTION_TIME = 5000".to_string(),
                "SET SESSION lock_wait_timeout = 17".to_string(),
            ],
        );
    }

    #[test]
    fn mysql_timeout_restore_capture_skips_only_unknown_variables() {
        assert!(matches!(
            MysqlExecutor::optional_timeout_restore_value("lock_wait_timeout", Ok(17)),
            Ok(Some(17))
        ));
        assert!(matches!(
            MysqlExecutor::optional_timeout_restore_value(
                "innodb_lock_wait_timeout",
                Err(MysqlError::MySqlError(MySqlError {
                    state: "HY000".to_string(),
                    code: 1193,
                    message: "Unknown system variable 'innodb_lock_wait_timeout'".to_string(),
                })),
            ),
            Ok(None)
        ));
        assert!(MysqlExecutor::optional_timeout_restore_value(
            "innodb_lock_wait_timeout",
            Err(MysqlError::DriverError(mysql::DriverError::SetupError)),
        )
        .is_err());
    }

    #[test]
    fn mysql_timeout_apply_skips_uncaptured_optional_lock_variables() {
        let restore = MysqlSessionTimeoutRestore {
            max_execution_time: Some(5000),
            max_statement_time: None,
            lock_wait_timeout: Some(17),
            innodb_lock_wait_timeout: None,
        };
        let mut conn = RecordingQueryable::default();

        MysqlExecutor::apply_session_timeout_for_db_with_restore_hint(
            &mut conn,
            Some(Duration::from_secs(5)),
            DatabaseType::MySQL,
            Some(&restore),
        )
        .expect("timeout application should skip uncaptured optional variables");

        assert_eq!(
            conn.statements,
            vec![
                "SET SESSION MAX_EXECUTION_TIME = 5000".to_string(),
                "SET SESSION lock_wait_timeout = 5".to_string(),
            ]
        );
    }

    fn mysql_test_env(name: &str) -> Option<String> {
        env::var(name).ok().filter(|value| !value.trim().is_empty())
    }

    fn mysql_timeout_restore_test_connection() -> Option<DatabaseConnection> {
        mysql_timeout_restore_test_connection_for_db_type(DatabaseType::MySQL)
    }

    fn mysql_timeout_restore_test_connection_for_db_type(
        db_type: DatabaseType,
    ) -> Option<DatabaseConnection> {
        let Some(host) = mysql_test_env("SPACE_QUERY_TEST_MYSQL_HOST") else {
            eprintln!("skipping: SPACE_QUERY_TEST_MYSQL_HOST is not set");
            return None;
        };
        let Some(database) = mysql_test_env("SPACE_QUERY_TEST_MYSQL_DATABASE") else {
            eprintln!("skipping: SPACE_QUERY_TEST_MYSQL_DATABASE is not set");
            return None;
        };
        let Some(user) = mysql_test_env("SPACE_QUERY_TEST_MYSQL_USER") else {
            eprintln!("skipping: SPACE_QUERY_TEST_MYSQL_USER is not set");
            return None;
        };
        let Some(password) = mysql_test_env("SPACE_QUERY_TEST_MYSQL_PASSWORD") else {
            eprintln!("skipping: SPACE_QUERY_TEST_MYSQL_PASSWORD is not set");
            return None;
        };
        let port = mysql_test_env("SPACE_QUERY_TEST_MYSQL_PORT")
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(3306);

        let mut connection = DatabaseConnection::new();
        connection
            .connect(ConnectionInfo::new_with_type(
                "MYSQL_TEST",
                &user,
                &password,
                &host,
                port,
                &database,
                db_type,
            ))
            .expect("MySQL/MariaDB timeout restore test connection should succeed");
        Some(connection)
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_session_timeout_restore_round_trips_real_session_variables() {
        assert_mysql_session_timeout_restore_round_trips_real_session_variables(
            DatabaseType::MySQL,
        );
    }

    #[test]
    #[ignore = "requires local MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mariadb_session_timeout_restore_round_trips_real_session_variables() {
        assert_mysql_session_timeout_restore_round_trips_real_session_variables(
            DatabaseType::MariaDB,
        );
    }

    fn assert_mysql_session_timeout_restore_round_trips_real_session_variables(
        db_type: DatabaseType,
    ) {
        let Some(mut connection) = mysql_timeout_restore_test_connection_for_db_type(db_type)
        else {
            return;
        };
        let db_type = connection.db_type();
        let conn = connection
            .get_mysql_connection_mut()
            .expect("MySQL test connection should be live");

        conn.query_drop("SET SESSION lock_wait_timeout = 17")
            .expect("set lock_wait_timeout test value");
        let innodb_supported = conn
            .query_drop("SET SESSION innodb_lock_wait_timeout = 123")
            .is_ok();
        let max_execution_time_supported = conn
            .query_drop("SET SESSION MAX_EXECUTION_TIME = 5000")
            .is_ok();
        let max_statement_time_supported = if max_execution_time_supported {
            false
        } else {
            conn.query_drop("SET SESSION max_statement_time = 2.500")
                .is_ok()
        };
        assert!(
            max_execution_time_supported || max_statement_time_supported,
            "MySQL/MariaDB test server should support a query timeout session variable"
        );

        let restore = MysqlExecutor::apply_session_timeout_with_restore_for_db(
            conn,
            Some(Duration::from_secs(1)),
            db_type,
        )
        .expect("timeout application should succeed")
        .expect("timeout restore should be captured when timeout is configured");
        conn.query_drop("SELECT 1")
            .expect("timed statement should still execute");
        restore
            .restore_for_db(conn, db_type)
            .expect("timeout restore should succeed for captured variables");

        assert_eq!(
            conn.query_first::<u64, _>("SELECT @@SESSION.lock_wait_timeout")
                .expect("read restored lock_wait_timeout"),
            Some(17)
        );
        if innodb_supported {
            assert_eq!(
                conn.query_first::<u64, _>("SELECT @@SESSION.innodb_lock_wait_timeout")
                    .expect("read restored innodb_lock_wait_timeout"),
                Some(123)
            );
        }
        if max_execution_time_supported {
            assert_eq!(
                conn.query_first::<u64, _>("SELECT @@SESSION.MAX_EXECUTION_TIME")
                    .expect("read restored MAX_EXECUTION_TIME"),
                Some(5000)
            );
        } else {
            let value = conn
                .query_first::<f64, _>("SELECT @@SESSION.max_statement_time")
                .expect("read restored max_statement_time")
                .expect("max_statement_time should be present");
            assert!((value - 2.5).abs() < f64::EPSILON);
        }
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_full_fetch_cancel_flag_returns_cancelled_result_and_reuses_connection() {
        let Some(mut connection) = mysql_timeout_restore_test_connection() else {
            return;
        };
        let db_type = connection.db_type();
        let conn = connection
            .get_mysql_connection_mut()
            .expect("MySQL test connection should be live");

        let mut cancel_checks = 0usize;
        let results = MysqlExecutor::execute_for_db_type_with_cancel(
            conn,
            "SELECT 1 AS n UNION ALL SELECT 2 AS n UNION ALL SELECT 3 AS n",
            db_type,
            || {
                cancel_checks += 1;
                cancel_checks >= 4
            },
        )
        .expect("cancel flag should produce a terminal QueryResult, not a driver error");

        let result = results
            .into_iter()
            .next()
            .expect("SELECT should produce one result");
        assert!(!result.success);
        assert!(result.is_select);
        assert_eq!(result.message, "Query cancelled");
        assert_eq!(result.row_count, 1);
        assert_eq!(result.rows, vec![vec!["1".to_string()]]);
        assert_eq!(
            conn.query_first::<u8, _>("SELECT 1")
                .expect("same MySQL connection should accept the next query"),
            Some(1)
        );
    }

    #[test]
    #[ignore = "requires local MySQL or MariaDB test database via SPACE_QUERY_TEST_MYSQL_* env vars"]
    fn mysql_cancel_running_query_interrupts_and_connection_can_query_next() {
        let Some(mut connection) = mysql_timeout_restore_test_connection() else {
            return;
        };
        let db_type = connection.db_type();
        let info = connection
            .runtime_connection_info()
            .expect("live MySQL connection should expose runtime connection info");
        let conn = connection
            .get_mysql_connection_mut()
            .expect("MySQL test connection should be live");
        let connection_id = conn.connection_id();
        let cancel_requested = Arc::new(AtomicBool::new(false));
        let cancel_requested_for_thread = cancel_requested.clone();

        let cancel_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            cancel_requested_for_thread.store(true, Ordering::SeqCst);
            MysqlExecutor::cancel_running_query(&info, connection_id)
        });

        let started = Instant::now();
        let result = MysqlExecutor::execute_for_db_type_with_cancel(
            conn,
            "SELECT SLEEP(10)",
            db_type,
            || cancel_requested.load(Ordering::SeqCst),
        );
        let cancel_result = cancel_thread
            .join()
            .expect("cancel thread should not panic");
        cancel_result.expect("KILL QUERY should be accepted by the server");

        match result {
            Ok(results) => {
                let result = results
                    .into_iter()
                    .next()
                    .expect("cancelled SELECT should produce one terminal result");
                assert!(!result.success);
                assert!(result.is_select);
                assert_eq!(result.message, "Query cancelled");
            }
            Err(err) => {
                assert!(
                    MysqlExecutor::is_cancel_error(&err),
                    "expected MySQL cancel error, got {err}"
                );
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "cancel took {:?}",
            started.elapsed()
        );
        assert_eq!(
            conn.query_first::<u8, _>("SELECT 1")
                .expect("same MySQL connection should accept the next query after KILL QUERY"),
            Some(1)
        );
    }

    #[test]
    fn mysql_cancel_connection_opts_do_not_require_current_database() {
        let opts = mysql::Opts::from(MysqlExecutor::build_cancel_opts(&ConnectionInfo {
            name: "local".to_string(),
            username: "user".to_string(),
            password: "secret".to_string(),
            host: "127.0.0.1".to_string(),
            port: 3306,
            service_name: "possibly_dropped_database".to_string(),
            db_type: DatabaseType::MySQL,
            advanced: crate::db::ConnectionAdvancedSettings::default_for(DatabaseType::MySQL),
            color: crate::db::ConnectionColor::default(),
            read_only: false,
            debug_oracle_thin_protocol_version: None,
        }));

        assert_eq!(opts.get_db_name(), None);
        assert_eq!(opts.get_tcp_connect_timeout(), Some(Duration::from_secs(2)));
        assert_eq!(opts.get_read_timeout(), Some(&Duration::from_secs(2)));
        assert_eq!(opts.get_write_timeout(), Some(&Duration::from_secs(2)));
    }

    #[test]
    fn mysql_cancel_treats_an_already_gone_thread_as_success() {
        let missing_thread = MysqlError::MySqlError(mysql::MySqlError {
            state: "HY000".to_string(),
            code: 1094,
            message: "Unknown thread id: 42".to_string(),
        });
        let permission_denied = MysqlError::MySqlError(mysql::MySqlError {
            state: "42000".to_string(),
            code: 1095,
            message: "You are not owner of thread 42".to_string(),
        });

        assert!(MysqlExecutor::normalize_cancel_result(Err(missing_thread)).is_ok());
        assert!(MysqlExecutor::normalize_cancel_result(Err(permission_denied)).is_err());
    }

    #[test]
    fn mysql_error_detection_distinguishes_timeout_from_cancel() {
        let timeout_err = MysqlError::MySqlError(MySqlError {
            state: "HY000".to_string(),
            code: 3024,
            message: "Query execution was interrupted, maximum statement execution time exceeded"
                .to_string(),
        });
        assert!(MysqlExecutor::is_timeout_error(&timeout_err));
        assert!(!MysqlExecutor::is_lock_wait_timeout_error(&timeout_err));
        assert!(!MysqlExecutor::is_cancel_error(&timeout_err));

        let mariadb_timeout_err = MysqlError::MySqlError(MySqlError {
            state: "70100".to_string(),
            code: 1969,
            message: "Query execution was interrupted (max_statement_time exceeded)".to_string(),
        });
        assert!(MysqlExecutor::is_timeout_error(&mariadb_timeout_err));
        assert!(!MysqlExecutor::is_lock_wait_timeout_error(
            &mariadb_timeout_err
        ));
        assert!(!MysqlExecutor::is_cancel_error(&mariadb_timeout_err));

        let lock_wait_err = MysqlError::MySqlError(MySqlError {
            state: "HY000".to_string(),
            code: 1205,
            message: "Lock wait timeout exceeded; try restarting transaction".to_string(),
        });
        assert!(MysqlExecutor::is_timeout_error(&lock_wait_err));
        assert!(MysqlExecutor::is_lock_wait_timeout_error(&lock_wait_err));
        assert!(!MysqlExecutor::is_cancel_error(&lock_wait_err));

        let cancel_err = MysqlError::MySqlError(MySqlError {
            state: "70100".to_string(),
            code: 1317,
            message: "Query execution was interrupted".to_string(),
        });
        assert!(MysqlExecutor::is_cancel_error(&cancel_err));
        assert!(!MysqlExecutor::is_timeout_error(&cancel_err));
    }

    #[test]
    fn mysql_materialize_call_results_preserves_dml_and_select_sets() {
        let results = MysqlExecutor::materialize_call_results(
            "CALL sync_and_list_users()",
            vec![
                MysqlResultSetSnapshot {
                    columns: Vec::new(),
                    rows: Vec::new(),
                    affected_rows: 2,
                    info: String::new(),
                },
                MysqlResultSetSnapshot {
                    columns: vec![ColumnInfo {
                        name: "user_name".to_string(),
                        data_type: "VARCHAR".to_string(),
                        kind: crate::db::SqlValueKind::Unknown,
                    }],
                    rows: vec![vec!["alice".to_string()], vec!["bob".to_string()]],
                    affected_rows: 0,
                    info: String::new(),
                },
                MysqlResultSetSnapshot {
                    columns: Vec::new(),
                    rows: Vec::new(),
                    affected_rows: 0,
                    info: String::new(),
                },
            ],
            Duration::from_millis(5),
        );

        assert_eq!(results.len(), 2);
        assert!(!results[0].is_select);
        assert_eq!(results[0].message, "CALL 2 row(s) affected");
        assert!(results[1].is_select);
        assert_eq!(results[1].row_count, 2);
        assert_eq!(results[1].columns[0].name, "user_name");
    }

    #[test]
    fn mysql_materialize_call_results_falls_back_to_call_executed_for_empty_ok_packets() {
        let results = MysqlExecutor::materialize_call_results(
            "CALL noop()",
            vec![MysqlResultSetSnapshot {
                columns: Vec::new(),
                rows: Vec::new(),
                affected_rows: 0,
                info: String::new(),
            }],
            Duration::from_millis(1),
        );

        assert_eq!(results.len(), 1);
        assert!(!results[0].is_select);
        assert_eq!(results[0].message, "Call executed successfully");
    }

    #[test]
    fn mysql_parse_routine_arguments_from_create_ddl_handles_procedure_signature() {
        let ddl = "CREATE DEFINER=`root`@`localhost` PROCEDURE `demo_proc`(IN p_id INT, INOUT `p_name` VARCHAR(50) DEFAULT 'guest', OUT p_total DECIMAL(10,2))\nBEGIN\n  SELECT 1;\nEND";

        let arguments =
            MysqlObjectBrowser::parse_routine_arguments_from_create_ddl(ddl, "PROCEDURE")
                .expect("procedure signature should parse");

        assert_eq!(arguments.len(), 3);
        assert_eq!(arguments[0].name.as_deref(), Some("p_id"));
        assert_eq!(arguments[0].in_out.as_deref(), Some("IN"));
        assert_eq!(arguments[0].data_type.as_deref(), Some("INT"));

        assert_eq!(arguments[1].name.as_deref(), Some("p_name"));
        assert_eq!(arguments[1].in_out.as_deref(), Some("INOUT"));
        assert_eq!(arguments[1].data_type.as_deref(), Some("VARCHAR(50)"));
        assert_eq!(arguments[1].default_value.as_deref(), Some("'guest'"));

        assert_eq!(arguments[2].name.as_deref(), Some("p_total"));
        assert_eq!(arguments[2].in_out.as_deref(), Some("OUT"));
        assert_eq!(arguments[2].data_type.as_deref(), Some("DECIMAL(10,2)"));
    }

    #[test]
    fn mysql_parse_routine_arguments_from_create_ddl_handles_function_return_type() {
        let ddl = "CREATE DEFINER=`root`@`localhost` FUNCTION `demo_func`(p_id INT, p_kind ENUM('A','B')) RETURNS VARCHAR(20) CHARACTER SET utf8mb4 DETERMINISTIC\nRETURN 'ok'";

        let arguments =
            MysqlObjectBrowser::parse_routine_arguments_from_create_ddl(ddl, "FUNCTION")
                .expect("function signature should parse");

        assert_eq!(arguments.len(), 3);
        assert_eq!(arguments[0].position, 0);
        assert_eq!(arguments[0].in_out.as_deref(), Some("RETURN"));
        assert_eq!(
            arguments[0].data_type.as_deref(),
            Some("VARCHAR(20) CHARACTER SET utf8mb4")
        );
        assert_eq!(arguments[1].name.as_deref(), Some("p_id"));
        assert_eq!(arguments[1].data_type.as_deref(), Some("INT"));
        assert_eq!(arguments[2].name.as_deref(), Some("p_kind"));
        assert_eq!(arguments[2].data_type.as_deref(), Some("ENUM('A','B')"));
    }

    // -----------------------------------------------------------------------
    // # comment handling in DDL parser helpers
    // -----------------------------------------------------------------------

    #[test]
    fn mysql_parse_routine_arguments_ignores_comma_inside_hash_comment() {
        // The hash comment on the first parameter contains a comma; it must not
        // be treated as a parameter separator.
        let ddl = "CREATE PROCEDURE `annotated_proc`(\
            p_id INT,    # first param, the user id\n\
            p_name VARCHAR(50)\
        )\nBEGIN SELECT 1; END";

        let arguments =
            MysqlObjectBrowser::parse_routine_arguments_from_create_ddl(ddl, "PROCEDURE")
                .expect("procedure with hash comments should parse");

        assert_eq!(
            arguments.len(),
            2,
            "comma inside # comment must not create a phantom parameter: {arguments:?}"
        );
        assert_eq!(arguments[0].name.as_deref(), Some("p_id"));
        assert_eq!(arguments[0].data_type.as_deref(), Some("INT"));
        assert_eq!(arguments[1].name.as_deref(), Some("p_name"));
        assert_eq!(arguments[1].data_type.as_deref(), Some("VARCHAR(50)"));
    }

    #[test]
    fn mysql_parse_routine_arguments_ignores_default_keyword_inside_hash_comment() {
        // DEFAULT inside a hash comment must not be mistaken for the parameter
        // default-value marker.
        let ddl = "CREATE PROCEDURE `commented_proc`(\
            p_status VARCHAR(20) # DEFAULT 'active' -- legacy default\n\
        )\nBEGIN SELECT 1; END";

        let arguments =
            MysqlObjectBrowser::parse_routine_arguments_from_create_ddl(ddl, "PROCEDURE")
                .expect("procedure with DEFAULT in hash comment should parse");

        assert_eq!(arguments.len(), 1);
        assert_eq!(arguments[0].name.as_deref(), Some("p_status"));
        assert_eq!(arguments[0].data_type.as_deref(), Some("VARCHAR(20)"));
        assert!(
            arguments[0].default_value.is_none(),
            "DEFAULT inside # comment must not be parsed as actual default value"
        );
    }

    #[test]
    fn mysql_parse_routine_arguments_hash_comment_with_paren_does_not_confuse_matching() {
        // A hash comment that contains parentheses must not disturb the paren-
        // matching logic used to locate the parameter list.
        let ddl = "CREATE FUNCTION `fn_hash_paren`(\
            p_val INT # range: (0, 100)\n\
        ) RETURNS INT DETERMINISTIC\nRETURN p_val * 2";

        let arguments =
            MysqlObjectBrowser::parse_routine_arguments_from_create_ddl(ddl, "FUNCTION")
                .expect("function with paren inside hash comment should parse");

        // First argument is the synthetic RETURN entry; second is p_val.
        assert!(
            arguments.iter().any(|a| a.name.as_deref() == Some("p_val")),
            "p_val parameter should be present: {arguments:?}"
        );
        assert!(
            arguments
                .iter()
                .any(|a| a.in_out.as_deref() == Some("RETURN")),
            "RETURN entry should be present: {arguments:?}"
        );
    }

    // -----------------------------------------------------------------------
    // USE statement db_name extraction
    // -----------------------------------------------------------------------

    #[test]
    fn mysql_extract_use_database_name_simple() {
        assert_eq!(MysqlExecutor::extract_use_database_name("USE mydb"), "mydb");
        assert_eq!(
            MysqlExecutor::extract_use_database_name("USE mydb;"),
            "mydb"
        );
    }

    #[test]
    fn mysql_is_use_statement_detects_only_use() {
        assert!(MysqlExecutor::is_use_statement("USE mydb"));
        assert!(MysqlExecutor::is_use_statement("/* switch */ USE `mydb`"));
        assert!(!MysqlExecutor::is_use_statement("SELECT * FROM users"));
        assert!(!MysqlExecutor::is_use_statement(
            "UPDATE users SET name = 'USE'"
        ));
    }

    #[test]
    fn mysql_use_statement_database_name_returns_selected_database() {
        assert_eq!(
            MysqlExecutor::use_statement_database_name("/* switch */ USE `my database`;")
                .as_deref(),
            Some("my database")
        );
        assert_eq!(
            MysqlExecutor::use_statement_database_name("-- switch\nUSE mydb").as_deref(),
            Some("mydb")
        );
        assert_eq!(MysqlExecutor::use_statement_database_name("SELECT 1"), None);
    }

    #[test]
    fn mysql_drop_database_statement_database_name_parses_drop_forms() {
        let name = |sql: &str| {
            MysqlExecutor::drop_database_statement_database_name_for_db_type(
                DatabaseType::MySQL,
                sql,
            )
        };
        assert_eq!(name("DROP DATABASE mydb").as_deref(), Some("mydb"));
        assert_eq!(name("drop schema mydb;").as_deref(), Some("mydb"));
        assert_eq!(
            name("DROP DATABASE IF EXISTS `my db`").as_deref(),
            Some("my db")
        );
        assert_eq!(
            name("  DROP\tSCHEMA  IF  EXISTS  mydb ;").as_deref(),
            Some("mydb")
        );
        // A database literally named `if` is not an IF EXISTS clause.
        assert_eq!(name("DROP DATABASE if").as_deref(), Some("if"));
        assert_eq!(name("DROP TABLE mydb"), None);
        assert_eq!(name("DROP DATABASE"), None);
        assert_eq!(name("SELECT 'DROP DATABASE mydb'"), None);
        assert_eq!(name("DROPDATABASE mydb"), None);
    }

    #[test]
    fn mysql_current_database_changed_message_is_concise() {
        assert_eq!(
            MysqlExecutor::current_database_changed_message("sales"),
            "Current database changed to sales."
        );
    }

    #[test]
    fn mysql_extract_use_database_name_backtick_quoted() {
        assert_eq!(
            MysqlExecutor::extract_use_database_name("USE `mydb`"),
            "mydb"
        );
        assert_eq!(
            MysqlExecutor::extract_use_database_name("USE `mydb`;"),
            "mydb"
        );
    }

    #[test]
    fn mysql_extract_use_database_name_backtick_with_spaces() {
        assert_eq!(
            MysqlExecutor::extract_use_database_name("USE `my database`"),
            "my database",
            "backtick-quoted name with spaces should be fully extracted"
        );
    }

    #[test]
    fn mysql_extract_use_database_name_backtick_with_escaped_backtick() {
        assert_eq!(
            MysqlExecutor::extract_use_database_name("USE `odd``name`"),
            "odd`name",
            "escaped backtick inside quoted name should be unescaped"
        );
    }

    #[test]
    fn mysql_extract_use_database_name_skips_leading_block_comments() {
        assert_eq!(
            MysqlExecutor::extract_use_database_name("USE /* switch */ mydb"),
            "mydb",
            "block comment between USE and db name should be ignored"
        );
        assert_eq!(
            MysqlExecutor::extract_use_database_name("USE /* first */ /* second */ `my database`;"),
            "my database",
            "multiple block comments before a quoted db name should be ignored"
        );
    }

    #[test]
    fn mysql_extract_use_database_name_skips_block_comments_before_use_keyword() {
        assert_eq!(
            MysqlExecutor::extract_use_database_name("/* preface */ USE mydb"),
            "mydb",
            "leading block comment before USE should be ignored"
        );
        assert_eq!(
            MysqlExecutor::extract_use_database_name(
                "  /* first */ /* second */ USE `my database`;"
            ),
            "my database",
            "multiple leading block comments before USE should be ignored"
        );
    }

    #[test]
    fn mysql_extract_use_database_name_skips_line_comments() {
        assert_eq!(
            MysqlExecutor::extract_use_database_name("-- preface\nUSE mydb"),
            "mydb",
            "leading dash comment before USE should be ignored"
        );
        assert_eq!(
            MysqlExecutor::extract_use_database_name("USE -- switch database\n`my database`;"),
            "my database",
            "dash comment between USE and db name should be ignored"
        );
        assert_eq!(
            MysqlExecutor::extract_use_database_name("# preface\nUSE mydb"),
            "mydb",
            "leading hash comment before USE should be ignored"
        );
        assert_eq!(
            MysqlExecutor::extract_use_database_name("USE # switch database\nmydb;"),
            "mydb",
            "hash comment between USE and db name should be ignored"
        );
    }

    // -----------------------------------------------------------------------
    // classify_statement additional coverage
    // -----------------------------------------------------------------------

    #[test]
    fn mysql_classify_statement_with_cte_select_is_select() {
        assert_eq!(
            MysqlExecutor::classify_statement(
                "WITH recent AS (SELECT 1 AS id) SELECT id FROM recent"
            ),
            super::MysqlStatementKind::Select,
            "WITH ... SELECT (CTE) should be classified as Select, not Dml"
        );
    }

    #[test]
    fn mysql_displayable_select_excludes_select_into_targets() {
        for sql in [
            "SELECT COUNT(*) INTO @cnt FROM orders",
            "SELECT total INTO @total FROM orders WHERE id = 1",
            "SELECT total INTO local_total FROM orders WHERE id = 1",
            "SELECT id, total INTO @id, @total FROM orders WHERE id = 1",
            "SELECT total FROM orders WHERE id = 1 INTO @total",
            "SELECT total FROM orders WHERE id = 1 INTO @total LOCK IN SHARE MODE",
            "SELECT 1 INTO @`odd-name`",
            "SELECT 1 INTO @'odd-name'",
            "SELECT 1 INTO @\"odd-name\"",
            "SELECT 1 INTO /* target comment */ @quoted_target FROM dual",
            "SELECT 1 INTO @plain_target, @`quoted-target`, local_target",
            "SELECT * INTO OUTFILE '/tmp/orders.tsv' FROM orders",
            "SELECT payload INTO DUMPFILE '/tmp/order.bin' FROM orders WHERE id = 1",
            "SELECT * FROM orders INTO OUTFILE '/tmp/orders.tsv'
             CHARACTER SET utf8mb4
             FIELDS TERMINATED BY ','
             LINES TERMINATED BY '\\n'",
            "WITH recent AS (SELECT 1 AS id) SELECT COUNT(*) INTO @cnt FROM recent",
            "WITH RECURSIVE tree AS (
                SELECT id, parent_id FROM categories WHERE parent_id IS NULL
                UNION ALL
                SELECT c.id, c.parent_id FROM categories c JOIN tree t ON c.parent_id = t.id
             ) SELECT COUNT(*) INTO @tree_count FROM tree",
            "WITH nested AS (SELECT 1 AS value)
             SELECT value FROM nested INTO @cte_value FOR UPDATE",
            "WITH recent AS (SELECT 1 AS id)
             SELECT id FROM recent UNION ALL SELECT id + 1 FROM recent INTO OUTFILE '/tmp/recent.tsv'",
            "(TABLE source_rows ORDER BY id LIMIT 2)
             UNION ALL (VALUES ROW(99, 'sentinel'))
             ORDER BY id LIMIT 1 INTO @picked_id, @picked_text",
            "/*!80000 SELECT COUNT(*) INTO @cnt FROM orders */",
        ] {
            assert!(
                MysqlExecutor::is_select_statement(sql),
                "SELECT ... INTO should still be a SELECT for session analysis: {sql}"
            );
            assert!(
                !MysqlExecutor::is_displayable_select_statement(sql),
                "SELECT ... INTO should not create a Data Grid tab: {sql}"
            );
        }

        assert!(
            !MysqlExecutor::is_displayable_select_statement_for_db_type(
                DatabaseType::MariaDB,
                "SET STATEMENT max_statement_time=1 FOR SELECT 1 INTO @wrapped_value",
            ),
            "MariaDB SET STATEMENT wrapping SELECT ... INTO should not create a Data Grid tab"
        );

        for sql in [
            "SELECT 1 AS value WHERE 0",
            "SELECT 'INTO @cnt' AS literal_text",
            r#"SELECT "INTO @cnt" AS quoted_literal_text"#,
            "SELECT `into` FROM `select_into_table`",
            "SELECT 1 AS `into`",
            "SELECT @`odd-name` AS quoted_user_variable",
            "SELECT @'odd-name' AS single_quoted_user_variable",
            "SELECT @\"odd-name\" AS double_quoted_user_variable",
            "SELECT into_col, count_into_value FROM orders",
            "SELECT JSON_EXTRACT(meta_json, '$.into') AS json_path FROM orders",
            "SELECT JSON_OBJECT('into', @value) AS payload",
            "SELECT /* INTO @cnt */ COUNT(*) FROM orders",
            "SELECT COUNT(*) FROM orders -- INTO @cnt",
            "WITH recent AS (SELECT 1 AS id) SELECT id FROM recent",
            "WITH recent AS (SELECT 'INTO @cnt' AS text_value)
             SELECT text_value FROM recent",
            "WITH nested AS (SELECT 1 INTO @cte_local)
             SELECT 2 AS display_value",
            "WITH nested AS (SELECT 1 INTO @`cte-local`)
             SELECT 2 AS display_value",
            "SELECT *
             FROM (
                SELECT 'INTO @nested' AS text_value
             ) nested_values",
            "SELECT *
             FROM orders
             WHERE EXISTS (
                SELECT 1 INTO @inner_target
             )",
            "EXPLAIN SELECT COUNT(*) INTO @cnt FROM orders",
            "EXPLAIN FORMAT=JSON SELECT COUNT(*) INTO @cnt FROM orders",
            "EXPLAIN WITH recent AS (SELECT 1 AS id) SELECT id INTO @id FROM recent",
            "SHOW WARNINGS",
            "DESCRIBE orders",
            "TABLE orders",
        ] {
            assert!(
                MysqlExecutor::is_displayable_select_statement(sql),
                "result-producing statements should remain displayable: {sql}"
            );
        }

        for sql in [
            "SET STATEMENT max_statement_time=1 FOR SELECT 1",
            "SET STATEMENT max_statement_time=1 FOR SELECT * FROM accounts FOR UPDATE",
        ] {
            assert!(
                MysqlExecutor::is_displayable_select_statement_for_db_type(
                    DatabaseType::MariaDB,
                    sql,
                ),
                "MariaDB SET STATEMENT wrapping result-producing SELECT should remain displayable: {sql}"
            );
            assert!(
                !MysqlExecutor::is_displayable_select_statement(sql),
                "MySQL mode should not treat MariaDB SET STATEMENT as displayable SELECT: {sql}"
            );
        }

        for sql in [
            "INSERT INTO orders_archive SELECT * FROM orders",
            "CREATE TABLE orders_copy AS SELECT * FROM orders",
            "REPLACE INTO orders_archive SELECT * FROM orders",
            "SET STATEMENT max_statement_time=1 FOR UPDATE orders SET total = total + 1",
        ] {
            assert!(
                !MysqlExecutor::is_displayable_select_statement(sql),
                "non-SELECT statements containing INTO/SELECT should not be displayable SELECTs: {sql}"
            );
        }
    }

    #[test]
    fn mariadb_set_statement_display_classification_uses_inner_statement() {
        for (sql, expected) in [
            (
                "SET STATEMENT max_statement_time=1 FOR SELECT 1",
                super::MysqlStatementKind::Select,
            ),
            (
                "SET STATEMENT max_statement_time=1 FOR SELECT * FROM accounts FOR UPDATE",
                super::MysqlStatementKind::Select,
            ),
            (
                "SET STATEMENT max_statement_time=1 FOR SELECT 1 INTO @wrapped_value",
                super::MysqlStatementKind::Select,
            ),
            (
                "SET STATEMENT max_statement_time=1 FOR UPDATE accounts SET balance = balance + 1",
                super::MysqlStatementKind::Dml,
            ),
            (
                "SET STATEMENT max_statement_time=1 FOR CALL sync_accounts()",
                super::MysqlStatementKind::Call,
            ),
        ] {
            assert_eq!(
                MysqlExecutor::classify_statement_for_db_type(DatabaseType::MariaDB, sql),
                expected,
                "{sql}"
            );
        }
        assert_eq!(
            MysqlExecutor::classify_statement("SET STATEMENT max_statement_time=1 FOR SELECT 1",),
            super::MysqlStatementKind::Ddl,
            "MySQL mode must not reinterpret MariaDB SET STATEMENT wrappers"
        );
    }

    #[test]
    fn mysql_classify_statement_replace_into_is_dml() {
        assert_eq!(
            MysqlExecutor::classify_statement("REPLACE INTO t(id, v) VALUES (1, 'x')"),
            super::MysqlStatementKind::Dml
        );
    }

    // -----------------------------------------------------------------------
    // Bug fix: SHOW / DESCRIBE / EXPLAIN must be routed as Select so that
    // their tabular result sets are not silently discarded by query_drop().
    // -----------------------------------------------------------------------

    #[test]
    fn mysql_classify_statement_show_databases_is_select() {
        assert_eq!(
            MysqlExecutor::classify_statement("SHOW DATABASES"),
            super::MysqlStatementKind::Select,
            "SHOW DATABASES must be Select so its result set is not discarded"
        );
    }

    #[test]
    fn mysql_classify_statement_show_tables_is_select() {
        assert_eq!(
            MysqlExecutor::classify_statement("SHOW TABLES"),
            super::MysqlStatementKind::Select
        );
        // Variants with qualifiers must also be Select.
        assert_eq!(
            MysqlExecutor::classify_statement("SHOW TABLES FROM mydb"),
            super::MysqlStatementKind::Select
        );
        assert_eq!(
            MysqlExecutor::classify_statement("SHOW FULL TABLES"),
            super::MysqlStatementKind::Select
        );
    }

    #[test]
    fn mysql_classify_statement_show_variables_is_select() {
        assert_eq!(
            MysqlExecutor::classify_statement("SHOW VARIABLES"),
            super::MysqlStatementKind::Select
        );
        assert_eq!(
            MysqlExecutor::classify_statement("SHOW VARIABLES LIKE 'sql_mode'"),
            super::MysqlStatementKind::Select
        );
    }

    #[test]
    fn mysql_classify_statement_show_status_is_select() {
        assert_eq!(
            MysqlExecutor::classify_statement("SHOW STATUS"),
            super::MysqlStatementKind::Select
        );
    }

    #[test]
    fn mysql_classify_statement_show_processlist_is_select() {
        assert_eq!(
            MysqlExecutor::classify_statement("SHOW PROCESSLIST"),
            super::MysqlStatementKind::Select
        );
        assert_eq!(
            MysqlExecutor::classify_statement("SHOW FULL PROCESSLIST"),
            super::MysqlStatementKind::Select
        );
    }

    #[test]
    fn mysql_classify_statement_show_warnings_and_errors_is_select() {
        assert_eq!(
            MysqlExecutor::classify_statement("SHOW WARNINGS"),
            super::MysqlStatementKind::Select
        );
        assert_eq!(
            MysqlExecutor::classify_statement("SHOW ERRORS"),
            super::MysqlStatementKind::Select
        );
    }

    #[test]
    fn mysql_classify_statement_show_create_is_select() {
        assert_eq!(
            MysqlExecutor::classify_statement("SHOW CREATE TABLE orders"),
            super::MysqlStatementKind::Select
        );
        assert_eq!(
            MysqlExecutor::classify_statement("SHOW CREATE PROCEDURE p_sync"),
            super::MysqlStatementKind::Select
        );
        assert_eq!(
            MysqlExecutor::classify_statement("SHOW CREATE VIEW v_active"),
            super::MysqlStatementKind::Select
        );
    }

    #[test]
    fn mysql_classify_statement_describe_is_select() {
        assert_eq!(
            MysqlExecutor::classify_statement("DESCRIBE employees"),
            super::MysqlStatementKind::Select,
            "DESCRIBE must be Select so its result set is not discarded"
        );
        assert_eq!(
            MysqlExecutor::classify_statement("DESC employees"),
            super::MysqlStatementKind::Select,
            "DESC must be Select so its result set is not discarded"
        );
    }

    #[test]
    fn mysql_classify_statement_explain_is_select() {
        assert_eq!(
            MysqlExecutor::classify_statement("EXPLAIN SELECT * FROM employees"),
            super::MysqlStatementKind::Select,
            "EXPLAIN must be Select so its result set is not discarded"
        );
        assert_eq!(
            MysqlExecutor::classify_statement("EXPLAIN UPDATE employees SET salary = 0"),
            super::MysqlStatementKind::Select,
            "EXPLAIN UPDATE must also be routed as Select"
        );
    }

    #[test]
    fn mysql_classify_statement_table_maintenance_commands_are_selects() {
        for sql in [
            "ANALYZE TABLE employees",
            "CHECK TABLE employees",
            "CHECKSUM TABLE employees",
            "OPTIMIZE TABLE employees",
            "REPAIR TABLE employees",
        ] {
            assert_eq!(
                MysqlExecutor::classify_statement(sql),
                super::MysqlStatementKind::Select,
                "{sql} returns a result set in MySQL/MariaDB and must not be discarded"
            );
        }
    }

    #[test]
    fn mysql_classify_statement_xa_recover_is_select() {
        assert_eq!(
            MysqlExecutor::classify_statement("XA RECOVER"),
            super::MysqlStatementKind::Select,
            "XA RECOVER returns prepared transaction rows and must not be discarded"
        );
        assert_eq!(
            MysqlExecutor::classify_statement("XA RECOVER CONVERT XID"),
            super::MysqlStatementKind::Select
        );
    }

    // -----------------------------------------------------------------------
    // Bug fix: parse_mysql_parameter must skip leading `--` style comments
    // just as it already skips leading `#` comments.
    // -----------------------------------------------------------------------

    #[test]
    fn mysql_parse_routine_arguments_ignores_comma_inside_dash_dash_comment() {
        // The `--` comment on the first parameter contains a comma; it must not
        // be treated as a parameter separator.
        let ddl = "CREATE PROCEDURE `dash_comment_proc`(\
            p_id INT,    -- first param, the user id\n\
            p_name VARCHAR(50)\
        )\nBEGIN SELECT 1; END";

        let arguments =
            MysqlObjectBrowser::parse_routine_arguments_from_create_ddl(ddl, "PROCEDURE")
                .expect("procedure with -- comments should parse");

        assert_eq!(
            arguments.len(),
            2,
            "comma inside -- comment must not create a phantom parameter: {arguments:?}"
        );
        assert_eq!(arguments[0].name.as_deref(), Some("p_id"));
        assert_eq!(arguments[0].data_type.as_deref(), Some("INT"));
        assert_eq!(arguments[1].name.as_deref(), Some("p_name"));
        assert_eq!(arguments[1].data_type.as_deref(), Some("VARCHAR(50)"));
    }

    #[test]
    fn mysql_parse_routine_arguments_skips_leading_dash_dash_comment_before_param() {
        // A `--` comment appears on its own line before the parameter name.
        // parse_mysql_parameter must skip it and still parse the name correctly.
        let ddl = "CREATE PROCEDURE `leading_dash_proc`(\
            -- user id\n\
            p_id INT,\
            -- user name\n\
            p_name VARCHAR(100)\
        )\nBEGIN SELECT 1; END";

        let arguments =
            MysqlObjectBrowser::parse_routine_arguments_from_create_ddl(ddl, "PROCEDURE")
                .expect("procedure with leading -- comments should parse");

        assert_eq!(
            arguments.len(),
            2,
            "leading -- comment must not break parameter parsing: {arguments:?}"
        );
        assert_eq!(arguments[0].name.as_deref(), Some("p_id"));
        assert_eq!(arguments[0].data_type.as_deref(), Some("INT"));
        assert_eq!(arguments[1].name.as_deref(), Some("p_name"));
        assert_eq!(arguments[1].data_type.as_deref(), Some("VARCHAR(100)"));
    }

    #[test]
    fn mysql_parse_routine_arguments_ignores_default_inside_dash_dash_comment() {
        // DEFAULT keyword appearing in a `--` comment must not be treated as
        // the parameter default-value marker.
        let ddl = "CREATE PROCEDURE `dash_default_proc`(\
            p_status VARCHAR(20) -- DEFAULT 'active'\n\
        )\nBEGIN SELECT 1; END";

        let arguments =
            MysqlObjectBrowser::parse_routine_arguments_from_create_ddl(ddl, "PROCEDURE")
                .expect("procedure with DEFAULT in -- comment should parse");

        assert_eq!(arguments.len(), 1);
        assert_eq!(arguments[0].name.as_deref(), Some("p_status"));
        assert_eq!(arguments[0].data_type.as_deref(), Some("VARCHAR(20)"));
        assert!(
            arguments[0].default_value.is_none(),
            "DEFAULT inside -- comment must not be parsed as actual default value"
        );
    }

    #[test]
    fn mysql_parse_routine_arguments_keeps_backslash_escaped_quote_inside_default_string() {
        let ddl = "CREATE PROCEDURE `escaped_default_proc`(\
            p_msg VARCHAR(50) DEFAULT 'it\\'s ok',\
            p_count INT\
        )\nBEGIN SELECT 1; END";

        let arguments =
            MysqlObjectBrowser::parse_routine_arguments_from_create_ddl(ddl, "PROCEDURE")
                .expect("procedure with backslash-escaped default string should parse");

        assert_eq!(
            arguments.len(),
            2,
            "escaped quote must not swallow the next parameter"
        );
        assert_eq!(arguments[0].name.as_deref(), Some("p_msg"));
        assert_eq!(arguments[0].default_value.as_deref(), Some("'it\\'s ok'"));
        assert_eq!(arguments[1].name.as_deref(), Some("p_count"));
    }

    #[test]
    fn mysql_parse_routine_arguments_keeps_double_dash_expression_when_no_comment_whitespace() {
        let ddl = "CREATE PROCEDURE `dash_expr_proc`(\
            p_score INT DEFAULT (5--2),\
            p_limit INT\
        )\nBEGIN SELECT 1; END";

        let arguments =
            MysqlObjectBrowser::parse_routine_arguments_from_create_ddl(ddl, "PROCEDURE")
                .expect("procedure with `--` arithmetic default should parse");

        assert_eq!(
            arguments.len(),
            2,
            "`--<non-space>` must stay part of the default expression"
        );
        assert_eq!(arguments[0].name.as_deref(), Some("p_score"));
        assert_eq!(arguments[0].default_value.as_deref(), Some("(5--2)"));
        assert_eq!(arguments[1].name.as_deref(), Some("p_limit"));
    }

    #[test]
    fn mysql_parse_routine_arguments_skips_leading_block_comment_before_param() {
        let ddl = "CREATE PROCEDURE `block_comment_proc`(\
            p_id INT,\
            /* second, user-facing parameter */ p_name VARCHAR(100)\
        )\nBEGIN SELECT 1; END";

        let arguments =
            MysqlObjectBrowser::parse_routine_arguments_from_create_ddl(ddl, "PROCEDURE")
                .expect("procedure with leading block comments should parse");

        assert_eq!(
            arguments.len(),
            2,
            "leading block comment must not hide the following parameter: {arguments:?}"
        );
        assert_eq!(arguments[0].name.as_deref(), Some("p_id"));
        assert_eq!(arguments[0].data_type.as_deref(), Some("INT"));
        assert_eq!(arguments[1].name.as_deref(), Some("p_name"));
        assert_eq!(arguments[1].data_type.as_deref(), Some("VARCHAR(100)"));
    }

    #[test]
    fn mysql_parse_routine_arguments_strips_inline_block_comment_from_type_section() {
        let ddl = "CREATE PROCEDURE `block_default_proc`(\
            p_status VARCHAR(20) /* keep legacy width */ DEFAULT 'active'\
        )\nBEGIN SELECT 1; END";

        let arguments =
            MysqlObjectBrowser::parse_routine_arguments_from_create_ddl(ddl, "PROCEDURE")
                .expect("procedure with inline block comment before DEFAULT should parse");

        assert_eq!(arguments.len(), 1);
        assert_eq!(arguments[0].name.as_deref(), Some("p_status"));
        assert_eq!(arguments[0].data_type.as_deref(), Some("VARCHAR(20)"));
        assert_eq!(arguments[0].default_value.as_deref(), Some("'active'"));
    }

    #[test]
    fn mysql_parse_routine_arguments_function_return_type_ignores_inline_block_comment() {
        let ddl = "CREATE FUNCTION `fn_block_comment_return`(p_id INT)\
            RETURNS VARCHAR(20) /* display label */ CHARACTER SET utf8mb4 DETERMINISTIC\n\
            RETURN 'ok'";

        let arguments =
            MysqlObjectBrowser::parse_routine_arguments_from_create_ddl(ddl, "FUNCTION")
                .expect("function return type with inline block comment should parse");

        assert_eq!(arguments[0].position, 0);
        assert_eq!(arguments[0].in_out.as_deref(), Some("RETURN"));
        assert_eq!(
            arguments[0].data_type.as_deref(),
            Some("VARCHAR(20) CHARACTER SET utf8mb4")
        );
    }
}
