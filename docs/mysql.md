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
session option to MySQL execution (pool) sessions from the executing tab's
value; the live metadata connection stays pinned to `autocommit=1` because it
never runs user SQL and an implicitly opened metadata transaction would
otherwise be reported by the dirty probe. Preflight rules for retained
sessions and transaction-mode changes are defined in the
[transaction document](transaction.md).

### Scope preparation runs once per acquisition

`acquire_mysql_pooled_session()` ends with a common tail that prepares the
session's database scope (`prepare_mysql_pooled_session_database()` —
`COM_INIT_DB` plus the encoding statements) and then applies the execution
settings (`ROLLBACK`, `SET autocommit`, `SET SESSION TRANSACTION ...`).

The branches that acquire a FRESH pooled session have already prepared the
scope through `prepare_mysql_pooled_session_or_retry_once()`, with the same
context and `preserve = false`. They therefore hand a `scope_already_prepared`
flag to the tail, which skips its own call — without it the identical
`COM_INIT_DB` + `SELECT DEFAULT_COLLATION_NAME` + `SET NAMES` trio went out
twice on every fresh session.

The tail still prepares the scope on a REUSED session, and still applies the
execution settings in both cases. Neither is redundant: a database can be
dropped while an idle pooled session still names it, and the tab's
transaction mode only reaches a reused session through those settings.

`prepare_mysql_pooled_session_database()` decides through
`mysql_pooled_session_scope_application()` and reports the database the session
ended up in, which is what the retained lease records:

- no work to protect — select the database and re-apply the session settings;
- work or residue, and the session is already in the target — touch nothing:
  `COM_INIT_DB` clears the diagnostics area, so `SHOW WARNINGS` after a DML
  would come back empty;
- work or residue, and the session is somewhere else (or its scope is unknown)
  — `USE` alone. `USE` neither commits nor rolls back, so the tab's transaction
  continues in the new database; a missing database is reported instead of
  falling back to "no database", because that fallback exists to keep a FRESH
  session usable and would throw away this one's work.

Skipping every preserved session instead — which is what "the retained session
already has the tracked scope" assumed — made the eager push a correctness
requirement it cannot meet (it needs the connection lock and gives up
silently). A tab whose push was missed ran every later statement in the old
database, and the lease still recorded the requested one, so nothing could
notice.

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

## Local test database

These development-only credentials connect to the repository's local MySQL
test container:

| Setting | Value |
| --- | --- |
| Container | `space-query-mysql80` |
| Host | `127.0.0.1` |
| Port | `3307` |
| Database | `query_tool_mysql8` |
| Username | `root` |
| Password | `spacequery` |

## Live tests

Connection tests read these variables:

```sh
export SPACE_QUERY_TEST_MYSQL_HOST=127.0.0.1
export SPACE_QUERY_TEST_MYSQL_PORT=3307
export SPACE_QUERY_TEST_MYSQL_DATABASE=query_tool_mysql8
export SPACE_QUERY_TEST_MYSQL_USER=root
export SPACE_QUERY_TEST_MYSQL_PASSWORD=spacequery
```

Verify the primary connection and pool session independently:

```sh
cargo test mysql_connect_applies_advanced_session_settings --lib -- --ignored --nocapture
cargo test mysql_pool_session_applies_advanced_session_settings --lib -- --ignored --nocapture
```

## Local formatter certification database

The repository has three destructive, self-contained formatter certification
fixtures:

- [`test_mysql/test4.txt`](../test_mysql/test4.txt) drops/recreates only
  `sq_mysql_format_cert`.
- [`test_mysql/test5.txt`](../test_mysql/test5.txt) drops/recreates only
  `sq_mysql_format_cert_2`.
- [`test_mysql/test6.txt`](../test_mysql/test6.txt) drops/recreates only
  `sq_mysql_format_cert_3`.

All require MySQL 8.0.31 or newer.

The fixtures were verified against this local test instance:

| Setting | Value |
| --- | --- |
| Server | MySQL 8.0.46 |
| Container | `space-query-mysql80` |
| Address | `127.0.0.1:3307` |
| Certification databases | `sq_mysql_format_cert`, `sq_mysql_format_cert_2`, `sq_mysql_format_cert_3` |
| Credentials | Container `root`; password read from `MYSQL_ROOT_PASSWORD` inside the container |

Run the SQL exactly as the client receives it:

```sh
docker exec -i space-query-mysql80 sh -lc \
  'mysql -uroot -p"$MYSQL_ROOT_PASSWORD" --show-warnings --binary-mode' \
  < test_mysql/test4.txt

docker exec -i space-query-mysql80 sh -lc \
  'mysql -uroot -p"$MYSQL_ROOT_PASSWORD" --show-warnings --binary-mode' \
  < test_mysql/test5.txt

docker exec -i space-query-mysql80 sh -lc \
  'mysql -uroot -p"$MYSQL_ROOT_PASSWORD" --show-warnings --binary-mode' \
  < test_mysql/test6.txt
```

Success requires exit status 0 and the final result row of each fixture to
report `PASS`. Together they cover routines and handlers, generated JSON
columns, recursive CTEs, `JSON_TABLE`, windows, set operators, CTE and
multi-table DML, transaction rollback, locking clauses, invisible
columns/indexes, functional and multi-valued JSON indexes, RANGE partitions,
`LATERAL`, `VALUES ROW`, `GROUPING`/`ROLLUP`, JSON schema checks, delimiter
traps, SRID-aware spatial data and indexes, full-text indexes and all three
search modes, row-alias upserts, LIST partitioning, optimizer histograms,
prepared spatial queries, standalone `TABLE`, `FOR SHARE SKIP LOCKED`, and SQL
assertions.

The formatter-side certification and full report sweep are:

```sh
cargo test format_sql_certifies_mysql_test4_gauntlet --lib
cargo test format_sql_certifies_mysql_test5_gauntlet --lib
cargo test format_sql_certifies_mysql_test6_gauntlet --lib
cargo test formatting_sweep_all_files_generate_out_report --lib -- --ignored --nocapture
```

Unit regressions do not require an external server:

```sh
cargo test mysql_advanced --lib
cargo test mysql_set_names --lib
cargo test session_time_zone_validation_matches_database_ranges --lib
```
