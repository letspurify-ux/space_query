# MariaDB Connection Differences

> Implementation: `src/db/connection.rs`,
> `src/db/query/execution_backend.rs`, `src/db/sql_classification.rs`

MariaDB has its own `DatabaseType::MariaDB`, but shares
`DatabaseBackendKind::MySql` and `SqlDialect::MySql`. Its connection form, SSL,
`sql_mode`, charset/collation, and pool-session setup follow the
[MySQL flow](mysql.md). This document records only actual implementation
differences.

## Why the concrete type is preserved

- MariaDB time-zone bounds are validated separately before saving.
- If the server version identifies MariaDB, the same bounds are checked again
  after connection.
- The inner SQL of MariaDB `SET STATEMENT ... FOR ...` is classified.
- MariaDB timeout and error markers use concrete-type policy.
- Pool sessions share `DbPoolSession::MySQL`, but their internal `db_type` must
  still be `MariaDB`.

Never collapse the runtime type to `DatabaseType::MySQL` merely because the
family is shared.

## Time-zone range

`mariadb_session_time_zone_in_range()` accepts `-12:59` through `+13:00`. The
MariaDB `MysqlBackend` instance uses this function and its own error message.

## Live tests

MariaDB tests read the `SPACE_QUERY_TEST_MYSQL_*` variables documented under
[MySQL live tests](mysql.md#live-tests). Point them at the MariaDB instance.

```sh
cargo test mariadb_connect_applies_advanced_session_settings --lib -- --ignored --nocapture
cargo test mariadb_pool_session_applies_advanced_session_settings --lib -- --ignored --nocapture
cargo test mysql_pool_session_applies_default_session_settings_from_local_mariadb --lib -- --ignored --nocapture
```

These regressions run without an external server:

```sh
cargo test mariadb_time_zone --lib
cargo test mariadb_set_statement --lib
cargo test --test db_dispatch_guards
```
