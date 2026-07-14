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

## Local formatter certification database

The repository has three destructive, self-contained formatter certification
fixtures:

- [`test_mariadb/test9.txt`](../test_mariadb/test9.txt) drops/recreates only
  `sq_mariadb_format_cert`.
- [`test_mariadb/test10.txt`](../test_mariadb/test10.txt) drops/recreates only
  `sq_mariadb_format_cert_2`.
- [`test_mariadb/test11.txt`](../test_mariadb/test11.txt) drops/recreates only
  `sq_mariadb_format_cert_3`.

All target MariaDB 12.2+.

The fixtures were verified against this local test instance:

| Setting | Value |
| --- | --- |
| Server | MariaDB 12.2.2 (`12.2.2-MariaDB-ubu2404`) |
| Container | `space-query-mariadb122` |
| Address | `127.0.0.1:3306` |
| Certification databases | `sq_mariadb_format_cert`, `sq_mariadb_format_cert_2`, `sq_mariadb_format_cert_3` |
| Credentials | Container `root`; password read from `MARIADB_ROOT_PASSWORD` inside the container |

Run the SQL exactly as the client receives it:

```sh
docker exec -i space-query-mariadb122 sh -lc \
  'mariadb -uroot -p"$MARIADB_ROOT_PASSWORD" --show-warnings --binary-mode' \
  < test_mariadb/test9.txt

docker exec -i space-query-mariadb122 sh -lc \
  'mariadb -uroot -p"$MARIADB_ROOT_PASSWORD" --show-warnings --binary-mode' \
  < test_mariadb/test10.txt

docker exec -i space-query-mariadb122 sh -lc \
  'mariadb -uroot -p"$MARIADB_ROOT_PASSWORD" --show-warnings --binary-mode' \
  < test_mariadb/test11.txt
```

Success requires exit status 0 and the final result row of each fixture to
report `PASS`. Together they cover sequences, system-versioned and
application-time tables, dynamic columns, parameterized and implicit cursor
`FOR` loops, anchored row types, reverse range loops, diagnostics,
`EXECUTE IMMEDIATE`, recursive `CYCLE`, `JSON_TABLE`, VECTOR data/indexes and
distance functions, UUID v7, INET6, `WITHOUT OVERLAPS`, `FOR PORTION OF` DML,
windows, temporal queries, `INTERSECT ALL`/`EXCEPT ALL`, `FETCH ... WITH TIES`,
DML `RETURNING`, simultaneous application/system-time tables, anonymous
`BEGIN NOT ATOMIC` blocks, temporal snapshot/range queries, direct `HANDLER`
reads, upsert `RETURNING`, Oracle-mode `MINUS`, `DELETE HISTORY`,
`LIMIT ... ROWS EXAMINED`, JSON normalization, `SET STATEMENT`, delimiter traps,
rollback behavior, and SQL assertions.

The formatter-side certification and full report sweep are:

```sh
cargo test format_sql_certifies_mariadb_test9_gauntlet --lib
cargo test format_sql_certifies_mariadb_test10_gauntlet --lib
cargo test format_sql_certifies_mariadb_test11_gauntlet --lib
cargo test formatting_sweep_all_files_generate_out_report --lib -- --ignored --nocapture
```

These regressions run without an external server:

```sh
cargo test mariadb_time_zone --lib
cargo test mariadb_set_statement --lib
cargo test --test db_dispatch_guards
```
