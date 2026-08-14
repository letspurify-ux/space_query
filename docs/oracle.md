# Oracle Connections and Verification

> Implementation: `src/db/connection.rs`, `crates/tns-thin`,
> `test_tns_thin.sh`

Oracle uses `DatabaseType::Oracle`, `DatabaseBackendKind::Oracle`, and
`SqlDialect::Oracle`. Two drivers are available: Thin and OCI.

## Connection modes

| Capability | Thin | OCI |
| --- | --- | --- |
| Implementation | Workspace `tns-thin` crate | `oracle` crate/ODPI-C |
| Oracle Client | Not required | Required |
| Direct address | Host/Port/Service | Host/Port/Service |
| TNS alias | Unsupported | Supported |
| Transport | TCP | TCP or TCPS |

`ConnectionAdvancedSettings::validate_oracle()` rejects a Thin TNS-alias
connection and Thin TCPS. For an OCI TNS alias, leave host empty and put the
alias in the service-name field; Oracle Net supplies the network settings.

## OCI client discovery

OCI mode calls `ensure_oracle_client_initialized()` and searches for the client
library in this order:

1. `ORACLE_CLIENT_LIB_DIR`
2. The platform library directory under `ORACLE_HOME`
3. `instantclient_*` directories under the platform roots returned by
   `oracle_client_search_roots()`

The application and client library must use the same CPU architecture. Thin
mode does not perform this discovery.

## Advanced settings

- OCI transport and SSL
- Thin/OCI driver mode
- Default transaction isolation and access mode
- Session time zone
- `NLS_DATE_FORMAT`
- `NLS_TIMESTAMP_FORMAT`

Oracle accepts offsets from `-12:00` through `+14:00`. NLS formats must be
non-empty and contain only characters accepted by the implementation.
`READ ONLY` can be combined with `Serializable` isolation (a read-only Oracle
transaction reads one consistent snapshot, which is exactly the serializable
guarantee, so the pair maps to `SET TRANSACTION READ ONLY`) but not with
`Read committed` — statement-level consistency cannot exist inside a
read-only transaction.

Settings are applied to both primary and pool sessions. Current schema is
tracked through `ALTER SESSION SET CURRENT_SCHEMA` and reapplied to acquired or
reused sessions. Transaction mode is not a session flag; `SET TRANSACTION` is
issued as the first statement of each transaction.

## Current schema is resolved by one rule, on both drivers

Every place that puts an Oracle session in a schema goes through
`DatabaseConnection::apply_oracle_current_schema_for_scope()` — the pooled
acquisition on both drivers, the per-statement assertion each batch makes, and
the thin lazy-fetch worker. The rule is total (the tab's scope, else the
connection's tracked schema, else the login user: applying "no schema" is a
no-op, and a recycled pooled session keeps whatever the last tab left on it)
and tolerant of a schema that has been dropped (ORA-01435 is logged and the
session carries on in the login schema).

Both halves matter. The OCI execution acquisition used to call the raw
`apply_oracle_current_schema()` instead, so dropping the schema a tab pointed
at made every statement on that tab fail with ORA-01435 — including the
statement that would have fixed it — while the same script on thin kept
working. Live check S46 in `verify_transaction_mode_live` pins it on all four
backends.

## A pooled session never carries another tab's session state

Oracle pools hand a session back exactly as its last user left it — unlike the
MySQL family, whose driver resets every returned connection
(`PoolOpts::reset_connection` is `true` by default, so `COM_RESET_CONNECTION`
runs on return). What makes an Oracle session safe to recycle is that
`oracle_session_setting_statements()` is applied on EVERY pool acquisition
(`DbConnectionPool::acquire_session_untracked`) and states the NLS formats and
— the one that matters here — `ALTER SESSION SET ISOLATION_LEVEL = <connection
default>`.

That last statement is what stops a session-level isolation from leaking
between tabs. `ALTER SESSION SET ISOLATION_LEVEL` is SESSION persistent, and
the reset in `oracle_transaction_mode_statements_for_tab()` is deliberately
issued only when a tab has actively selected the default isolation ("a tab that
never touched the controls has adopted nothing and pays nothing") — so a tab
that pinned nothing would otherwise inherit whatever the previous user of that
physical session left on it.

For that to hold, the level the pool states has to be a CONCRETE one.
`TransactionIsolation::Default` has no `sql_level()`, so a pool still holding
it prepares its sessions with no isolation statement at all — "leave the
session wherever the last tab left it" — and a connection whose advanced
*Default transaction isolation* is left at `Default` (the first entry of that
dropdown) hit exactly that. `sync_default_transaction_isolation()` therefore
resolves the level (configured, else read from the server, else the backend
fallback) and records it on the pool in one step
(`DbConnectionPool::set_session_default_transaction_isolation`), so `Default`
can never reach session preparation. Live checks S47 (configured level) and
S51 (`Default`) in `verify_transaction_mode_live` pin this on both drivers with
a pool of exactly one session, and assert the tab really received the same
physical session (same `SID`) so they cannot pass by never meeting the hazard.

Isolation and `CURRENT_SCHEMA` are the only two `ALTER SESSION SET` targets
that need this. They are also the only two the residue classifier treats as
leaving a session CLEAN (`statement_session_post_processor_for`): every other
target — `TIME_ZONE` included — sets `may_leave_unknown_state`, which makes the
session `requires_physical_session_preservation()`, so it stays with its tab
and is discarded at close rather than returned to the pool. A setting that can
travel back into the pool is exactly a setting session preparation must state
totally.

## Local test database

These development-only credentials connect to the repository's local Oracle
test container:

| Setting | Value |
| --- | --- |
| Container | `oracle` |
| Host | `127.0.0.1` |
| Port | `1521` |
| Service name | `FREE` |
| Username | `system` |
| Password | `password` |

## Live tests

Ignored tests in the main crate read:

```sh
export ORACLE_TEST_HOST=127.0.0.1
export ORACLE_TEST_PORT=1521
export ORACLE_TEST_SERVICE_NAME=FREE
export ORACLE_TEST_USERNAME=system
export ORACLE_TEST_PASSWORD=password
```

OCI tests additionally require `ORACLE_CLIENT_LIB_DIR`. TNS-alias tests require
`TNS_ADMIN` and `ORACLE_TEST_TNS_ALIAS`. Use the repository script for the full
Thin/OCI live suite and protocol comparison instead of duplicating that command
matrix here:

```sh
./test_tns_thin.sh --help
./test_tns_thin.sh
```

Individual main-crate connection tests can also be run:

```sh
cargo test oracle_thin_connect_applies_advanced_session_settings_from_local_xe --lib -- --ignored --nocapture
cargo test oracle_thin_pool_session_applies_advanced_session_settings_from_local_xe --lib -- --ignored --nocapture
```

Run the TCPS test only where an OCI TCPS listener is configured.
