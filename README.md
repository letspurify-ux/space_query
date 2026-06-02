# SPACE Query

SPACE Query is a desktop SQL client built with Rust and FLTK. It supports Oracle, MySQL, and MariaDB connections, and bundles a SQL editor, script execution, an object browser, a result grid, query history, a session activity view, and log/crash diagnostics into a single app.

## Supported Databases

- Oracle
  - Supports a choice between Thin mode (built-in TNS client) and OCI (thick) mode
  - Thin mode connects without Oracle Instant Client; OCI mode requires the Instant Client or Full Client
  - Supports Host/Port/Service connections and TNS alias connections
  - Supports TCP/TCPS, NLS date/timestamp format, session time zone, and default transaction option settings
- MySQL
  - Supports database selection, SSL options, SQL mode, charset/collation, and session time zone settings
- MariaDB
  - Shares the MySQL-family execution backend and SQL dialect, but is managed separately as a distinct database type
  - Separates the MariaDB time zone range and some message handling

## Key Features

### Connections and Sessions

- Managed list of saved connections
- Passwords are stored in the OS Keyring rather than in the config file
- Per-connection advanced option validation
- Maintains a recent SQL file list
- Connection-pool-based session acquisition
- Lazy fetch for long queries and per-result-tab session tracking
- Checks for running queries and lazy fetch state before switching connections, disconnecting, or commit/rollback
- View the current connection/pool/result-tab state under Tools > Session Activity

### SQL Editor

- Multiple SQL tabs
- New SQL file, open, save, save as, close
- Syntax highlighting
- IntelliSense popup
- Find / Replace
- SQL formatting
- Comment toggle
- Uppercase/lowercase conversion of the selection
- Select and execute the current statement
- Execute the selection
- Execute the entire script
- Execution timeout input
- Previous/next navigation through query history
- Quick Describe at the cursor position

### Execution

- `F5`: Run script
- `Ctrl+Enter`, `F9`: Run current statement
- `F4`: Quick Describe
- `F6`: Explain Plan / EXPLAIN
- `F7`: Commit
- `F8`: Rollback
- Tools > Auto-Commit toggle
- Oracle bind variable, `PRINT`, and ref cursor result handling
- MySQL/MariaDB `SHOW`, `DESC`, `EXPLAIN` result set handling
- Displays execution results and messages in separate result tabs

### Script and Tool Commands

Script execution uses a dedicated parser together with session state, not simple semicolon splitting.

Oracle / SQL*Plus family:

- `VAR`, `VARIABLE`, `PRINT`
- `SET SERVEROUTPUT`
- `SET DEFINE`, `SET SCAN`, `SET VERIFY`, `SET ECHO`, `SET TIMING`, `SET FEEDBACK`, `SET HEADING`
- `SET PAGESIZE`, `SET LINESIZE`, `SET TRIMSPOOL`, `SET TRIMOUT`, `SET SQLBLANKLINES`, `SET TAB`, `SET COLSEP`, `SET NULL`
- `SHOW ERRORS`, `SHOW USER`, `SHOW ALL`
- `DESC`, `DESCRIBE`
- `PROMPT`, `PAUSE`, `ACCEPT`
- `DEFINE`, `UNDEFINE`, `COLUMN ... NEW_VALUE`
- `BREAK`, `COMPUTE`, `CLEAR BREAKS`, `CLEAR COMPUTES`
- `SPOOL`
- `WHENEVER SQLERROR`, `WHENEVER OSERROR`
- `@`, `@@`, `START`
- `CONNECT`, `DISCONNECT`, `EXIT`, `QUIT`

MySQL / MariaDB family:

- `USE`
- `SHOW DATABASES`, `SHOW TABLES`, `SHOW COLUMNS`
- `SHOW CREATE TABLE`
- `SHOW PROCESSLIST`, `SHOW VARIABLES`, `SHOW STATUS`
- `SHOW WARNINGS`, `SHOW ERRORS`
- `DELIMITER`
- `SOURCE`

### Object Browser

- Displays different root categories depending on the database type
- Filterable tree UI
- Object refresh
- Table/view data query
- Structure view
- Index view
- Constraint view
- DDL generation
- Package routine display

Oracle root categories:

- Tables
- Views
- Procedures
- Functions
- Sequences
- Triggers
- Synonyms
- Packages

MySQL / MariaDB root categories:

- Tables
- Views
- Procedures
- Functions
- Triggers
- Events
- Sequences are shown only when actually detected

### Result View

- Separate data and message tabs
- Per-result-tab status display
- CSV export
- Copy selected cells
- Copy with headers
- Configurable maximum cell preview length
- Configurable lazy fetch batch size
- Lazy fetch additional fetch, fetch all, and cancel
- `ROWID`-based staged edits for Oracle single-table result sets
  - Insert
  - Delete
  - Save
  - Cancel
  - Set Null

Result grid editing is not available for every SELECT. The current implementation assumes a safely identifiable Oracle single-table result set, and does not treat JOIN results or results to which a `ROWID` cannot be reliably attached as editable.

### Settings, Logs, and Recovery

- UI/editor/result font settings
- Result cell preview length setting
- Lazy fetch batch size setting
- Connection pool size setting
- App settings persistence
- App log viewer
- Log export and clear
- Panic-hook-based `crash.log` recording
- Displays the previous crash report on the next run
- Migration of the legacy `oracle_query_tool` config/keyring namespace

## Requirements

- Rust toolchain (stable) — required when building from source.
- Supported platforms: macOS, Linux, Windows.

## Running

This workspace contains multiple binaries, so you must specify `--bin space_query` when running.

Development run:

```bash
cargo run --bin space_query
```

Release run:

```bash
cargo run --release --bin space_query
```

## Building

Distribution binaries are produced with a release build.

```bash
cargo build --release --bin space_query
```

The output is created at `target/release/space_query` (`space_query.exe` on Windows).

## Testing

Full test suite:

```bash
cargo test
```

Build check:

```bash
cargo check
```

Some tests may require an external DB or environment variables. When running real Oracle/MySQL/MariaDB connection tests, you must first set up a local DB, account, and client library configuration.

## Oracle Client (OCI mode)

Thin mode connects without any additional client. An Oracle Instant Client or Full Client is required only in OCI (thick) mode, where the client library is auto-discovered in the following order.

1. The directory pointed to by the `ORACLE_CLIENT_LIB_DIR` environment variable
2. The `ORACLE_HOME` environment variable (Windows: `%ORACLE_HOME%\bin`, Linux/macOS: `$ORACLE_HOME/lib`)
3. The `instantclient_*` directory in platform-specific default locations
   - macOS: `/opt/oracle`
   - Linux: `/opt/oracle`, `/usr/local/oracle`
   - Windows: `C:\oracle`, `%ProgramFiles%\Oracle`

If auto-discovery does not work, set `ORACLE_CLIENT_LIB_DIR` directly.

```bash
export ORACLE_CLIENT_LIB_DIR=/opt/oracle/instantclient_23_3
cargo run --release --bin space_query
```

On Apple Silicon, the app and the client library must have the same CPU architecture.

### TNS alias connections

TNS alias connections are supported only in OCI mode, and alias resolution is performed by Oracle Net reading `tnsnames.ora`. Set the `TNS_ADMIN` environment variable to the directory containing `tnsnames.ora`. If it is not set, `$ORACLE_HOME/network/admin` is used; the Instant Client does not have this default path, so `TNS_ADMIN` is effectively required.

```bash
export TNS_ADMIN=/opt/oracle/network/admin
```

Thin mode supports only Host/Port/Service connections and does not support TNS aliases.

## Linux Build Notes

Running the GUI requires FLTK/X11 runtime dependencies. Before building, install the relevant development packages (`libxinerama`, `libxcursor`, `libxfixes`, `libxft`, etc.).

## Storage Locations

The OS-specific root of each path follows that OS's standard directory conventions, and the app directory name is `space_query`.

- Config file: `config_dir()/space_query/config.json`
- App log: `data_dir()/space_query/app.log.json`
- Crash log: `data_dir()/space_query/crash.log`
- Passwords: the `space_query` service in the OS Keyring

Notes:

- Connection information and the recent SQL file list are stored in the config file.
- Passwords are not stored in the config JSON.
- Query history is managed in the memory of the currently running app process.

## License and Trademarks

This project is distributed under `MIT OR Apache-2.0`. See `LICENSE-MIT` and `LICENSE-APACHE` for the full licenses.

The TNS thin implementation references the permissive-licensed implementations of `python-oracledb` and `go-ora`. The relevant notices are maintained in `THIRD_PARTY_NOTICES.md` and `crates/tns-thin/THIRD_PARTY_NOTICES.md`.

Oracle, Java, MySQL, and NetSuite are registered trademarks of Oracle and/or its affiliates. Other names may be trademarks of their respective owners. This project is not affiliated with, endorsed by, or sponsored by Oracle.
