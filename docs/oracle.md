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
`READ ONLY` cannot be combined with explicit isolation.

Settings are applied to both primary and pool sessions. Current schema is
tracked through `ALTER SESSION SET CURRENT_SCHEMA` and reapplied to acquired or
reused sessions. Transaction mode is not a session flag; `SET TRANSACTION` is
issued as the first statement of each transaction.

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
