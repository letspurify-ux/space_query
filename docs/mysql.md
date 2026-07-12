# MySQL Connections and Verification

> Implementation: `src/db/connection.rs`,
> `src/db/query/execution_backend.rs`, `src/db/sql_classification.rs`

MySQL uses `DatabaseType::MySQL`, `DatabaseBackendKind::MySql`, and
`SqlDialect::MySql`. It shares an execution family and SQL dialect with MariaDB,
while preserving the concrete database type through connection validation and
error/timeout policy.

## Connection settings

The default form uses host `localhost`, port `3306`, and an optional database.
It has no TNS alias or driver-mode selector. Advanced settings store:

- SSL mode and CA path
- Default transaction isolation and access mode
- Session time zone
- `sql_mode`
- Character set and optional collation

Defaults are `sql_mode=TRADITIONAL`, `charset=utf8mb4`, and
`session_time_zone=+00:00`. `ConnectionAdvancedSettings` and
`MysqlBackend::default_advanced_settings()` are authoritative.

## Values applied to sessions

After connecting and whenever a pool session is acquired,
`apply_mysql_session_settings_for_db_type()` applies:

```sql
SET SESSION sql_mode = ...
SET SESSION time_zone = ...
SET SESSION TRANSACTION ISOLATION LEVEL ...
SET NAMES <charset> [COLLATE <collation>]
```

Encoding and transaction options are reapplied after switching databases or
returning to an empty database scope. Auto-commit is applied as an actual
session option to MySQL live and execution sessions. Preflight rules for
retained sessions and transaction-mode changes are defined in the
[transaction document](transaction.md).

## Input validation

- A time zone is blank or has `+HH:MM`/`-HH:MM` form.
- The MySQL offset range is `-13:59` through `+14:00`.
- `sql_mode`, charset, and collation accept only the allowed ASCII identifier
  characters.
- A collation must match its charset. The `utf8`/`utf8mb3` alias combinations
  and `binary`/`binary` are explicitly accepted.
- SSL supports `Disabled`, `Required`, `VerifyCa`, and `VerifyIdentity`.

The authoritative implementations are `mysql_session_time_zone_in_range()`,
`ConnectionAdvancedSettings::validate_mysql()`, and
`mysql_collation_matches_charset()`.

## Live tests

Connection tests read these variables:

```sh
export SPACE_QUERY_TEST_MYSQL_HOST=127.0.0.1
export SPACE_QUERY_TEST_MYSQL_PORT=3306
export SPACE_QUERY_TEST_MYSQL_DATABASE=database_name
export SPACE_QUERY_TEST_MYSQL_USER=user_name
export SPACE_QUERY_TEST_MYSQL_PASSWORD=password
```

Verify the primary connection and pool session independently:

```sh
cargo test mysql_connect_applies_advanced_session_settings --lib -- --ignored --nocapture
cargo test mysql_pool_session_applies_advanced_session_settings --lib -- --ignored --nocapture
```

Unit regressions do not require an external server:

```sh
cargo test mysql_advanced --lib
cargo test mysql_set_names --lib
cargo test session_time_zone_validation_matches_database_ranges --lib
```
