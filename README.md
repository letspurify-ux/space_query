# SPACE Query

SPACE Query is a desktop SQL client for Oracle, MySQL, and MariaDB, built with
Rust and FLTK. It brings connection management, object-aware SQL editing,
script execution, result inspection, and session diagnostics into one app.

![SPACE Query main window](docs/images/main-window.png)

## Highlights

- Connect through Oracle Thin or OCI, MySQL, and MariaDB profiles.
- Edit SQL with syntax highlighting, IntelliSense, function signature hints,
  formatting, quick describe, file tabs, and query history.
- Run one statement, a selection, or a complete database-aware script.
- Inspect, sort, copy, export, and lazily fetch query results in independent
  tabs.
- Browse tables with IntelliSense-aware WHERE and ORDER BY expressions and
  bounded, database-side paging.
- Track active sessions, view application logs, and recover crash details.

## Database support

| Database | Connection and session support |
| --- | --- |
| Oracle | Built-in Thin mode over TCP, or OCI mode over TCP/TCPS. Both accept Host/Port/Service; OCI also supports TNS aliases. Advanced options include NLS date/timestamp formats, session time zone, and default transaction behavior. |
| MySQL | Optional database selection, SSL settings, SQL mode, charset/collation, session time zone, and transaction options. |
| MariaDB | A distinct database type using the MySQL-family SQL dialect and execution backend, with MariaDB-specific time-zone validation and message handling. |

Oracle Thin mode does not require Oracle Instant Client. See
[Oracle connection modes](#oracle-connection-modes) when using OCI or a TNS
alias.

## Install and run

### Prebuilt releases

[GitHub Releases](https://github.com/letspurify-ux/space_query/releases)
currently provide archives for macOS arm64 and Windows x86_64. Extract the
archive and run `space_query` (`space_query.exe` on Windows).

The current archives are not code-signed. Use the supplied checksums and
provenance attestation to verify them as described in
[Release verification](#release-verification).

### Build from source

Source builds support macOS, Linux, and Windows. The Rust version is pinned in
`rust-toolchain.toml` and is installed automatically by `rustup`.

On Debian or Ubuntu, install the FLTK/X11 development packages first:

```bash
sudo apt-get install libx11-dev libxext-dev libxft-dev libxinerama-dev \
  libxcursor-dev libxrender-dev libxfixes-dev
```

This repository contains multiple binaries, so specify the application binary:

```bash
# Development build
cargo run --bin space_query

# Optimized build and run
cargo run --release --bin space_query
```

To create only the distribution executable:

```bash
cargo build --release --bin space_query
```

The output is `target/release/space_query` on macOS/Linux and
`target/release/space_query.exe` on Windows.

## Getting started

### 1. Connect

Open **File > Connect** (`Ctrl+N`), select a database type, enter the connection
details, and then test, save, or open the connection. Saved passwords go to the
OS keyring rather than the configuration file.

![Database connection dialog](docs/images/connection-dialog.png)

- For Oracle, choose Thin or OCI. Thin uses Host, Port, and Service without an
  external client.
- For MySQL or MariaDB, enter Host, Port, Username, Password, and an optional
  database, then adjust SSL or session options if necessary.

### 2. Edit and execute SQL

Write SQL in the active query tab, then use the toolbar or a shortcut. On
macOS, use `Cmd` where `Ctrl` is shown.

| Action | Shortcut |
| --- | --- |
| Execute the selection or statement at the cursor | `Ctrl+Enter` or `F9` |
| Execute the complete script | `F5` |
| Quick describe the object at the cursor | `F4` |
| Explain Plan / EXPLAIN | `F6` |
| Commit / roll back | `F7` / `F8` |
| Open IntelliSense | `Ctrl+Space` |
| Format the selection or current statement | `Ctrl+Shift+F` |

The complete shortcut list is available under **Help > Keyboard Shortcuts**.

### 3. Inspect results

The lower workspace keeps each output type separate:

- **Data Grid** shows query rows and Explain Plan / EXPLAIN results, with
  selection, copy, CSV export, sorting, and lazy-fetch controls.
- **Script Output** and **DBMS Output** retain script transcripts and server
  output.
- **Messages** reports execution details, affected-row counts, and errors.

Use **File > Disconnect** (`Ctrl+D`) when finished. Before disconnecting,
switching connections, committing, or rolling back, SPACE Query asks you to
resolve any running query, lazy fetch, transaction, or pending grid edit that
cannot be closed safely.

## Features

### SQL editor and IntelliSense

![SQL IntelliSense suggestions](docs/images/intellisense.png)

IntelliSense uses the current SQL context and loaded database metadata to
suggest keywords, schemas, tables, views, aliases, columns, routines, packages,
and other objects. Use the arrow keys to select an item, `Enter` or `Tab` to
insert it, and `Esc` to close the popup.

![Function signature hint with the active argument emphasized](docs/images/signature-popup.png)

When the cursor is inside a function or procedure call, the signature hint
shows the available parameters and emphasizes the active argument. It follows
typing and mouse cursor movement, and closes when the application window moves
or resizes so it cannot remain detached from the editor.

The editor also supports multiple SQL file tabs, open/save/recent files,
syntax highlighting, find and replace, undo/redo, comment toggling, selection
case conversion, SQL block selection, execution timeouts, and previous/next
query-history navigation.

### SQL formatting

| Before | After `Ctrl+Shift+F` |
| --- | --- |
| ![SQL before automatic formatting](docs/images/sql-formatting-before.png) | ![SQL after automatic formatting](docs/images/sql-formatting-after.png) |

The formatter applies SQL-aware line breaks and indentation in place. It uses
the Oracle or MySQL/MariaDB dialect path of the active connection.

### Script execution

Script execution uses a dedicated parser and retained session state rather
than splitting text on every semicolon. Oracle bind variables, `PRINT`, and ref
cursors can produce results; MySQL/MariaDB `SHOW`, `DESC`, and `EXPLAIN`
statements are also handled as result sets. **Tools > Auto-Commit** controls
automatic transaction commits for the active connection.

<details>
<summary>Supported script and tool commands</summary>

Oracle / SQL*Plus family:

- `VAR`, `VARIABLE`, `PRINT`
- `SET SERVEROUTPUT`
- `SET DEFINE`, `SET SCAN`, `SET VERIFY`, `SET ECHO`, `SET TIMING`,
  `SET FEEDBACK`, `SET HEADING`
- `SET PAGESIZE`, `SET LINESIZE`, `SET TRIMSPOOL`, `SET TRIMOUT`,
  `SET SQLBLANKLINES`, `SET TAB`, `SET COLSEP`, `SET NULL`
- `SHOW ERRORS`, `SHOW USER`, `SHOW ALL`
- `DESC`, `DESCRIBE`
- `PROMPT`, `PAUSE`, `ACCEPT`
- `DEFINE`, `UNDEFINE`, `COLUMN ... NEW_VALUE`
- `BREAK ON <column>`, `BREAK OFF`
- `COMPUTE SUM`, `COMPUTE COUNT`, `COMPUTE OFF`, optionally with
  `OF <column> ON <group_column>`
- `CLEAR BREAKS`, `CLEAR COMPUTES`
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

</details>

### Object browser

![Object browser with example Oracle metadata](docs/images/object-browser.png)

The filterable object tree supports refresh, data queries, structure/index/
constraint inspection, DDL generation, and package routine browsing. Oracle
groups tables, views, procedures, functions, sequences, triggers, synonyms,
and packages. MySQL/MariaDB groups tables, views, procedures, functions,
triggers, and events, and shows sequences when the server exposes them.

### Table browsing and bounded paging

![Table browser with WHERE, ORDER BY, and paging controls](docs/images/table-browse.png)

Double-click a table in the object browser to open its rows in a dedicated
result tab. The filter bar above the grid keeps the **WHERE** and **ORDER BY**
editors at equal widths as the window resizes. Enter expressions without the
`WHERE` or `ORDER BY` keywords, press `Enter` to apply them, or use the clear
button to reset a field.

The same metadata-aware IntelliSense used by the SQL editor is available in
both fields. Start typing or press `Ctrl+Space` (`Cmd+Space` on macOS); the
suggestion popup opens directly below the text cursor in the active field. It
moves above the field or stays within the screen edge when space is limited.

![WHERE IntelliSense positioned at the text cursor](docs/images/table-browse-popup.png)

Use the first, previous, next, and last controls below the grid and choose a
page size of 10, 100, 250, 500, or 1,000 rows. Table browsing issues a bounded
query for each page—Oracle uses `ROWNUM`, while MySQL and MariaDB use
`LIMIT/OFFSET`—and does not retain a lazy-fetch cursor between pages. Completed
table tabs show `Page N` with the current page row count, and row headers retain
their absolute positions across pages.

### Result grid

![Result grid with a multi-cell selection](docs/images/result-grid.png)

- Drag or use the keyboard to select cells; `Ctrl+C` copies the selection and
  `Ctrl+Shift+C` includes headers.
- Resize columns, sort by a column header, or export the grid as CSV with
  `Ctrl+E`.
- Configure the maximum cell preview length and lazy-fetch batch size. Scrolling
  near the end fetches more rows, while full-result actions can fetch all
  remaining rows first.
- Use the context menu to close a result, copy data, export CSV, or access
  available edit actions.

For a safely identifiable Oracle, MySQL, or MariaDB single-table result,
**Edit** mode can stage inserted, updated, deleted, or `NULL` values. Oracle
uses `ROWID`; MySQL and MariaDB use a primary key or a non-null unique key.
Changes reach the database only after **Save**. MySQL/MariaDB locks each
existing target and checks its original values before mutation; Oracle uses
guarded `ROWID` DML with the same one-row rule. The whole save is rolled back
on a conflict. JOINs, multi-table results, and results without a reliable row
identifier remain read-only.

![Oracle result grid in staged edit mode](docs/images/result-grid-editing.png)

### Settings and diagnostics

![Application settings](docs/images/settings.png)

**Settings > Preferences** controls editor/result fonts, global UI size, result
preview and lazy-fetch limits, connection-pool size, cancellation behavior, and
how many query-history and application-log entries are retained. Settings
persist between launches.

![Query history with SQL and error preview](docs/images/query-history.png)

**Tools > Query History** searches executed statements, filters failures, shows
SQL and error details, and sends a selected statement back to the editor.
History is kept in a file and restored at the next launch; **Settings >
Preferences > History & Log** sets how many query-history and application-log
entries are retained.

![Session activity result](docs/images/session-activity.png)

**Tools > Session Activity** shows active and retained sessions, running SQL,
lazy fetches, result-tab ownership, fetched-row counts, and elapsed time.

![Application log viewer](docs/images/application-log.png)

**Tools > Application Log** filters, inspects, exports, and clears log entries.
If the app panics, it records `crash.log` and displays that report at the next
launch.

## Oracle connection modes

| Capability | Thin | OCI (thick) |
| --- | --- | --- |
| External Oracle client | Not required | Instant Client or Full Client required |
| Address | Host / Port / Service | Host / Port / Service or TNS alias |
| Transport | TCP | TCP or TCPS |

### OCI client discovery

The app searches for an OCI client in this order:

1. `ORACLE_CLIENT_LIB_DIR`
2. `ORACLE_HOME` (`%ORACLE_HOME%\bin` on Windows or `$ORACLE_HOME/lib` on
   macOS/Linux)
3. An `instantclient_*` directory under the platform defaults:
   - macOS: `/opt/oracle`
   - Linux: `/opt/oracle`, `/usr/local/oracle`
   - Windows: `C:\oracle`, `%ProgramFiles%\Oracle`

If discovery fails, set the library directory explicitly:

```bash
export ORACLE_CLIENT_LIB_DIR=/opt/oracle/instantclient_23_3
cargo run --release --bin space_query
```

On Apple Silicon, the app and OCI client must use the same CPU architecture.

### TNS aliases

TNS aliases are available only in OCI mode. Point `TNS_ADMIN` to the directory
containing `tnsnames.ora`:

```bash
export TNS_ADMIN=/opt/oracle/network/admin
```

Without `TNS_ADMIN`, Oracle Net checks `$ORACLE_HOME/network/admin`. Instant
Client has no equivalent default, so it normally requires `TNS_ADMIN` for alias
connections.

## Local data

SPACE Query uses the standard OS-specific roots returned by the Rust `dirs`
library:

| Data | Location |
| --- | --- |
| Settings, connection profiles, recent SQL files | `config_dir()/space_query/config.json` |
| Application log | `data_dir()/space_query/app.log.json` |
| Crash report | `data_dir()/space_query/crash.log` |
| Saved passwords | `space_query` service in the OS keyring |
| Query history | `data_dir()/space_query/query_history.json` |

Passwords are never written to `config.json`. Existing data in the legacy
`oracle_query_tool` config and keyring namespaces is migrated when encountered.

## Development

### Checks and tests

Run the non-live checks with the pinned toolchain:

```bash
cargo check --locked --bin space_query
cargo test --locked
cargo test --locked --manifest-path crates/tns-thin/Cargo.toml
```

The `tns-thin` crate is a path dependency rather than a workspace member, so it
has a separate test command. Live database tests require a configured local
database, credentials, and any applicable client libraries. Oracle Thin live
and comparison tests are available through:

```bash
./test_tns_thin.sh
```

Pull requests and pushes to `main` run formatting, Clippy, both non-live test
suites, and Linux x86_64, macOS arm64, and Windows x86_64 build checks in GitHub
Actions.

### Documentation

| Document | Purpose |
| --- | --- |
| [`docs/oracle.md`](docs/oracle.md) | Oracle development and live-test setup |
| [`docs/mysql.md`](docs/mysql.md) | MySQL development and live-test setup |
| [`docs/mariadb.md`](docs/mariadb.md) | MariaDB development and live-test setup |
| [`docs/session.md`](docs/session.md) | Session ownership, cancellation, and lazy-fetch rules |
| [`docs/transaction.md`](docs/transaction.md) | Transaction and retained-session behavior |
| [`docs/result_ui.md`](docs/result_ui.md) | Result tabs and grid behavior |
| [`docs/formatting.md`](docs/formatting.md) | SQL formatter structure and invariants |
| [`docs/highlighting.md`](docs/highlighting.md) | Syntax-highlighting pipeline |
| [`docs/new_backend.md`](docs/new_backend.md) | Checklist for adding a database backend |

### Regenerate feature screenshots

On macOS, regenerate all images under `docs/images` with:

```bash
./scripts/capture_feature_tour.sh
```

Pass an image name to capture only that scene, for example:

```bash
./scripts/capture_feature_tour.sh object-browser
```

The capture implementation is in `src/bin/capture_feature_tour.rs` and uses
isolated application settings.

## Release verification

Each GitHub Release contains `SHA256SUMS` for the macOS and Windows archives.
Verify it on Linux:

```bash
sha256sum --check SHA256SUMS
```

Or on macOS:

```bash
shasum -a 256 --check SHA256SUMS
```

Release archives also have GitHub artifact provenance attestations. With an
authenticated GitHub CLI, verify an archive with:

```bash
gh attestation verify space_query-macos-arm64.zip \
  --repo letspurify-ux/space_query
```

Checksums verify archive integrity, and attestations verify the GitHub Actions
build origin. Neither replaces Apple Developer ID or Windows Authenticode code
signing.

Release archives include the executable, `DISCLAIMER.md`, and a `licenses/`
directory containing the SPACE Query licenses, third-party notices and
dependency licenses, `tns-thin` provenance, referenced upstream notices, and
the copyright text for the Rust toolchain used to build the binary.

## License and trademarks

Original SPACE Query code is available under `MIT OR Apache-2.0`; see
[`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE). The bundled
`tns-thin` crate is Apache-2.0 licensed. Its MIT license file covers only the
identified `go-ora` material.

The software is provided without warranty and remains subject to
[`DISCLAIMER.md`](DISCLAIMER.md). Third-party attribution and exact upstream
revisions are recorded in [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md),
[`crates/tns-thin/THIRD_PARTY_NOTICES.md`](crates/tns-thin/THIRD_PARTY_NOTICES.md),
and [`crates/tns-thin/PROVENANCE.md`](crates/tns-thin/PROVENANCE.md).

Oracle, Java, MySQL, and NetSuite are registered trademarks of Oracle and/or
its affiliates. Other names may be trademarks of their respective owners. This
project is not affiliated with, endorsed by, or sponsored by Oracle.
