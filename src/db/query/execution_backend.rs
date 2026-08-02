use std::time::Duration;

use crate::db::sql_classification::{SqlKind, SqlStatementAnalysis};
use crate::db::DatabaseType;

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
    fn profile_statement(&self, sql: &str) -> StatementExecutionProfile;

    fn query_timeout_for_statement(
        &self,
        sql: &str,
        query_timeout: Option<Duration>,
    ) -> Option<Duration>;
}

struct OracleExecutionBackend;
struct MysqlExecutionBackend {
    db_type: DatabaseType,
}

impl DbExecutionBackend for OracleExecutionBackend {
    fn db_type(&self) -> DatabaseType {
        DatabaseType::Oracle
    }

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

impl DbExecutionBackend for MysqlExecutionBackend {
    fn db_type(&self) -> DatabaseType {
        self.db_type
    }

    fn profile_statement(&self, sql: &str) -> StatementExecutionProfile {
        statement_execution_profile_for_db_type(self.db_type, sql)
    }

    fn query_timeout_for_statement(
        &self,
        sql: &str,
        query_timeout: Option<Duration>,
    ) -> Option<Duration> {
        query_timeout_for_statement_for_db_type(self.db_type, sql, query_timeout)
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
    let result_kind = match db_type {
        DatabaseType::Oracle => classify_oracle_result_kind(sql),
        DatabaseType::MySQL => classify_mysql_result_kind(db_type, sql),
        DatabaseType::MariaDB => classify_mysql_result_kind(db_type, sql),
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
    let skips_session_timeout_wrapper = match db_type {
        DatabaseType::Oracle => false,
        DatabaseType::MySQL => {
            mysql_statement_sets_session_timeout_variable(sql)
                || mysql_statement_uses_timeout_sensitive_diagnostics(db_type, sql)
        }
        DatabaseType::MariaDB => {
            mysql_statement_sets_session_timeout_variable(sql)
                || mysql_statement_uses_timeout_sensitive_diagnostics(db_type, sql)
        }
    };
    if query_timeout.is_some() && skips_session_timeout_wrapper {
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

fn mysql_statement_uses_timeout_sensitive_diagnostics(db_type: DatabaseType, sql: &str) -> bool {
    let analysis = SqlStatementAnalysis::new_for_db_type(db_type, sql);
    analysis.classify_for_db_type(db_type) == SqlKind::Dml
        || analysis.words().iter().any(|word| {
            matches!(
                word.as_str(),
                "SQL_CALC_FOUND_ROWS" | "FOUND_ROWS" | "ROW_COUNT"
            )
        })
}

fn classify_mysql_result_kind(db_type: DatabaseType, sql: &str) -> StatementResultKind {
    let normalized = QueryExecutor::normalize_sql_for_execute(sql);
    if normalized.is_empty() {
        return StatementResultKind::Empty;
    }
    let display_sql = match db_type {
        DatabaseType::MariaDB => {
            crate::db::sql_classification::mariadb_set_statement_inner_sql(&normalized)
                .unwrap_or_else(|| normalized.clone())
        }
        DatabaseType::MySQL => normalized.clone(),
        DatabaseType::Oracle => return classify_oracle_result_kind(&normalized),
    };
    if QueryExecutor::is_select_statement(&display_sql) {
        return StatementResultKind::Select;
    }

    let analysis = SqlStatementAnalysis::new_for_db_type(db_type, &display_sql);
    if analysis.classify_for_db_type(db_type).is_select_like() {
        return StatementResultKind::Select;
    }
    let is_mariadb_compound_statement = match db_type {
        DatabaseType::MariaDB => analysis.starts_with_words(&["BEGIN", "NOT", "ATOMIC"]),
        DatabaseType::MySQL | DatabaseType::Oracle => false,
    };
    if is_mariadb_compound_statement {
        return StatementResultKind::Call;
    }
    if analysis.starts_with_words(&["ALTER", "TABLE"])
        && analysis.words().windows(2).any(|words| {
            matches!(
                words,
                [first, second] if first == "ANALYZE" && second == "PARTITION"
            )
        })
    {
        return StatementResultKind::Dml;
    }

    match QueryExecutor::leading_keyword(&display_sql).as_deref() {
        Some("INSERT") | Some("UPDATE") | Some("DELETE") | Some("REPLACE") | Some("WITH") => {
            StatementResultKind::Dml
        }
        // These MySQL-family statements may or may not return columns based on
        // their target. The DML execution route materializes optional result
        // sets without inventing a grid for OPEN/CLOSE or non-query payloads.
        Some("EXECUTE") | Some("HANDLER") | Some("HELP") | Some("CACHE") => {
            StatementResultKind::Dml
        }
        Some("LOAD") if analysis.starts_with_words(&["LOAD", "INDEX", "INTO", "CACHE"]) => {
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
    fn mysql_profile_materializes_result_set_optional_statements() {
        for sql in [
            "EXECUTE prepared_query",
            "HANDLER source_rows READ FIRST",
            "HELP 'SELECT'",
        ] {
            let profile = statement_execution_profile_for_db_type(DatabaseType::MariaDB, sql);
            assert_eq!(
                profile.result_kind,
                StatementResultKind::Dml,
                "unexpected profile for {sql}"
            );
        }
    }

    #[test]
    fn mariadb_profile_materializes_compound_and_admin_result_sets() {
        let cases = [
            ("BEGIN NOT ATOMIC SELECT 1; END", StatementResultKind::Call),
            (
                "ALTER TABLE t ANALYZE PARTITION p0",
                StatementResultKind::Dml,
            ),
            (
                "CACHE INDEX t KEY (PRIMARY) IN cache",
                StatementResultKind::Dml,
            ),
            ("LOAD INDEX INTO CACHE t", StatementResultKind::Dml),
        ];
        for (sql, expected) in cases {
            let profile = statement_execution_profile_for_db_type(DatabaseType::MariaDB, sql);
            assert_eq!(
                profile.result_kind, expected,
                "unexpected profile for {sql}"
            );
        }
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

    #[test]
    fn mysql_timeout_profile_preserves_statement_diagnostics() {
        let timeout = Some(Duration::from_secs(60));

        for db_type in [DatabaseType::MySQL, DatabaseType::MariaDB] {
            for sql in [
                "SELECT SQL_CALC_FOUND_ROWS id FROM t LIMIT 2",
                "SET @found = FOUND_ROWS()",
                "UPDATE t SET value = value + 1",
                "SET @updated = ROW_COUNT()",
            ] {
                assert_eq!(
                    query_timeout_for_statement_for_db_type(db_type, sql, timeout),
                    None,
                    "{db_type}: {sql}"
                );
            }
        }
        assert_eq!(
            query_timeout_for_statement_for_db_type(
                DatabaseType::MariaDB,
                "SELECT 'FOUND_ROWS() / ROW_COUNT()' AS text_value",
                timeout,
            ),
            timeout
        );
    }
}
