use std::time::Duration;

use crate::db::sql_classification::{SqlKind, SqlStatementAnalysis};
use crate::db::{DatabaseBackendKind, DatabaseType};

use super::QueryExecutor;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatementResultKind {
    Empty,
    Select,
    Dml,
    Commit,
    Rollback,
    Use,
    Call,
    Exec,
    Ddl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatementExecutionProfile {
    pub result_kind: StatementResultKind,
    pub session_kind: SqlKind,
}

pub trait DbExecutionBackend: Sync {
    fn db_type(&self) -> DatabaseType;

    fn profile_statement(&self, sql: &str) -> StatementExecutionProfile {
        statement_execution_profile_for_db_type(self.db_type(), sql)
    }

    fn query_timeout_for_statement(
        &self,
        sql: &str,
        query_timeout: Option<Duration>,
    ) -> Option<Duration> {
        query_timeout_for_statement_for_db_type(self.db_type(), sql, query_timeout)
    }
}

struct OracleExecutionBackend;
struct MysqlExecutionBackend {
    db_type: DatabaseType,
}

impl DbExecutionBackend for OracleExecutionBackend {
    fn db_type(&self) -> DatabaseType {
        DatabaseType::Oracle
    }
}

impl DbExecutionBackend for MysqlExecutionBackend {
    fn db_type(&self) -> DatabaseType {
        self.db_type
    }
}

static ORACLE_EXECUTION_BACKEND: OracleExecutionBackend = OracleExecutionBackend;
static MYSQL_EXECUTION_BACKEND: MysqlExecutionBackend = MysqlExecutionBackend {
    db_type: DatabaseType::MySQL,
};
static MARIADB_EXECUTION_BACKEND: MysqlExecutionBackend = MysqlExecutionBackend {
    db_type: DatabaseType::MariaDB,
};

pub fn db_execution_backend_for(db_type: DatabaseType) -> &'static dyn DbExecutionBackend {
    match db_type {
        DatabaseType::Oracle => &ORACLE_EXECUTION_BACKEND,
        DatabaseType::MySQL => &MYSQL_EXECUTION_BACKEND,
        DatabaseType::MariaDB => &MARIADB_EXECUTION_BACKEND,
    }
}

pub fn statement_execution_profile_for_db_type(
    db_type: DatabaseType,
    sql: &str,
) -> StatementExecutionProfile {
    let analysis = SqlStatementAnalysis::new_for_db_type(db_type, sql);
    let session_kind = analysis.classify_for_db_type(db_type);
    let result_kind = match db_type.backend_kind() {
        DatabaseBackendKind::Oracle => classify_oracle_result_kind(sql),
        DatabaseBackendKind::MySql => classify_mysql_result_kind(db_type, sql),
    };
    StatementExecutionProfile {
        result_kind,
        session_kind,
    }
}

pub fn query_timeout_for_statement_for_db_type(
    db_type: DatabaseType,
    sql: &str,
    query_timeout: Option<Duration>,
) -> Option<Duration> {
    if query_timeout.is_some()
        && db_type.backend_kind() == DatabaseBackendKind::MySql
        && mysql_statement_sets_session_timeout_variable(sql)
    {
        None
    } else {
        query_timeout
    }
}

fn mysql_statement_sets_session_timeout_variable(sql: &str) -> bool {
    crate::db::sql_classification::mysql_set_statement_assigns_session_variable(
        sql,
        &[
            "LOCK_WAIT_TIMEOUT",
            "INNODB_LOCK_WAIT_TIMEOUT",
            "MAX_EXECUTION_TIME",
            "MAX_STATEMENT_TIME",
        ],
    )
}

fn classify_mysql_result_kind(db_type: DatabaseType, sql: &str) -> StatementResultKind {
    let normalized = QueryExecutor::normalize_sql_for_execute(sql);
    if normalized.is_empty() {
        return StatementResultKind::Empty;
    }
    let display_sql = if db_type == DatabaseType::MariaDB {
        crate::db::sql_classification::mariadb_set_statement_inner_sql(&normalized)
            .unwrap_or_else(|| normalized.clone())
    } else {
        normalized.clone()
    };
    if QueryExecutor::is_select_statement(&display_sql) {
        return StatementResultKind::Select;
    }

    let analysis = SqlStatementAnalysis::new_for_db_type(db_type, &display_sql);
    if analysis.classify_for_db_type(db_type).is_select_like() {
        return StatementResultKind::Select;
    }

    match QueryExecutor::leading_keyword(&display_sql).as_deref() {
        Some("INSERT") | Some("UPDATE") | Some("DELETE") | Some("REPLACE") | Some("WITH") => {
            StatementResultKind::Dml
        }
        Some("COMMIT") => StatementResultKind::Commit,
        Some("ROLLBACK") => StatementResultKind::Rollback,
        Some("USE") => StatementResultKind::Use,
        Some("CALL") => StatementResultKind::Call,
        Some("SHOW") | Some("DESCRIBE") | Some("DESC") | Some("EXPLAIN") | Some("ANALYZE")
        | Some("CHECK") | Some("CHECKSUM") | Some("OPTIMIZE") | Some("REPAIR") => {
            StatementResultKind::Select
        }
        Some("XA")
            if SqlStatementAnalysis::new_for_db_type(db_type, &display_sql)
                .starts_with_words(&["XA", "RECOVER"]) =>
        {
            StatementResultKind::Select
        }
        _ => StatementResultKind::Ddl,
    }
}

fn classify_oracle_result_kind(sql: &str) -> StatementResultKind {
    let normalized = QueryExecutor::normalize_sql_for_execute(sql);
    if normalized.is_empty() {
        return StatementResultKind::Empty;
    }
    if QueryExecutor::is_select_statement(&normalized) {
        return StatementResultKind::Select;
    }

    match QueryExecutor::leading_keyword(&normalized).as_deref() {
        Some("INSERT") | Some("UPDATE") | Some("DELETE") | Some("MERGE") => {
            StatementResultKind::Dml
        }
        Some("BEGIN") | Some("DECLARE") | Some("CALL") => StatementResultKind::Call,
        Some("EXEC") | Some("EXECUTE") => StatementResultKind::Exec,
        Some("COMMIT") if QueryExecutor::is_plain_commit(&normalized) => {
            StatementResultKind::Commit
        }
        Some("ROLLBACK") if QueryExecutor::is_plain_rollback(&normalized) => {
            StatementResultKind::Rollback
        }
        _ => StatementResultKind::Ddl,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mysql_profile_separates_display_route_from_session_safety() {
        let profile =
            statement_execution_profile_for_db_type(DatabaseType::MySQL, "ANALYZE TABLE t");

        assert_eq!(profile.result_kind, StatementResultKind::Select);
        assert_eq!(profile.session_kind, SqlKind::Ddl);
    }

    #[test]
    fn mariadb_profile_unwraps_set_statement_for_result_and_session_kind() {
        let profile = statement_execution_profile_for_db_type(
            DatabaseType::MariaDB,
            "SET STATEMENT max_statement_time=1 FOR SELECT 1",
        );

        assert_eq!(profile.result_kind, StatementResultKind::Select);
        assert_eq!(profile.session_kind, SqlKind::SelectLike);
    }

    #[test]
    fn mysql_profile_uses_concrete_db_type_for_xa_recover() {
        let profile = statement_execution_profile_for_db_type(DatabaseType::MariaDB, "XA RECOVER");

        assert_eq!(profile.result_kind, StatementResultKind::Select);
        assert_eq!(profile.session_kind, SqlKind::SelectLike);
    }

    #[test]
    fn mysql_timeout_profile_skips_wrapper_for_session_timeout_set() {
        let timeout = Some(Duration::from_secs(5));

        assert_eq!(
            query_timeout_for_statement_for_db_type(
                DatabaseType::MariaDB,
                "SET SESSION max_statement_time = 1",
                timeout,
            ),
            None
        );
        assert_eq!(
            query_timeout_for_statement_for_db_type(
                DatabaseType::MariaDB,
                "SET @max_statement_time = 1",
                timeout,
            ),
            timeout
        );
    }
}
