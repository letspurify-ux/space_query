# SPACE Query

A desktop SQL client for Oracle, MySQL, and MariaDB. One native binary, no
runtime, no browser, no background service — and on Oracle, no client library to
install first.

![SPACE Query main window](docs/images/main-window.png)

## Why SPACE Query

**Oracle without an Instant Client.** Thin mode speaks Oracle's network protocol
directly, so an extracted binary is enough to connect, run a script, and read
the result. OCI mode stays available for TCPS and TNS aliases; nothing about the
rest of the app changes when you switch.

**Three databases, one set of habits.** Oracle, MySQL, and MariaDB share the
same editor, object browser, result grid, and diagnostics. The SQL dialect,
metadata queries, transaction rules, and error handling follow the connection
you are on.

**It refuses to guess.** Values are rendered from the column types the driver
reported, not from how they look. An execution plan's tree is drawn from the
parent each step actually reported. A local sort compares numbers by their
digits, so a 38-digit Oracle `NUMBER` keeps every one of them. A drop shows you
the exact statement before it runs, and runs that statement — nothing widened on
your behalf.

**It says what it is doing.** Session activity, query history, and an
application log show what every connection is up to. Disconnecting, committing,
or switching connections stops for anything still unresolved instead of
discarding it quietly.

**It is checked, not assumed.** The repository carries over 8,000 in-tree tests
— the patch number in the app's version *is* the exact count — plus a set of
verification binaries that drive the real UI and real servers, including
export→import round trips on every format and backend, and a read-only
connection proven against a writable control group.

## What you get

| Area | |
| --- | --- |
| **Editor** | Metadata-aware completion, signature hints, code snippets, SQL-aware formatter, multiple file tabs, find and replace, soft wrap, go to line, quick describe, go to declaration |
| **Execution** | A statement, a selection, or a whole script with SQL\*Plus-style commands, bind variables, ref cursors, and per-statement timeouts |
| **Results** | Independent result tabs, lazy fetch, sorting, in-grid search, exact selection totals, staged in-grid editing, and export to CSV/TSV/JSON/XML/HTML/Markdown/SQL |
| **Objects** | Filterable tree, structure/index/constraint inspection, DDL generation, confirmed drop and truncate, table browsing with database-side paging, file import |
| **Operations** | Session activity, persisted query history, application log, crash reports, and unsaved-tab recovery |

The full behavior of each feature — including the edges and the documented
limits — is in the [feature reference](docs/features.md).

## Database support

| Database | Connection and session support |
| --- | --- |
| **Oracle** | Built-in Thin mode over TCP, or OCI mode over TCP/TCPS. Both accept Host/Port/Service; OCI also supports TNS aliases. NLS date/timestamp formats, session time zone, and default transaction behavior are configurable. |
| **MySQL** | Optional database selection, SSL settings, SQL mode, charset/collation, session time zone, and transaction options. |
| **MariaDB** | A distinct database type on the MySQL-family dialect and execution backend, with MariaDB-specific time-zone validation and message handling. |

Thin mode needs no Oracle client. See [Oracle connection
modes](#oracle-connection-modes) for OCI and TNS aliases.

## Install

### Download a release

[GitHub Releases](https://github.com/letspurify-ux/space_query/releases)
currently provide archives for macOS arm64 and Windows x86_64. Extract and run
`space_query` (`space_query.exe` on Windows).

The archives are not code-signed. Verify them with the published checksums and
provenance attestation — see [Release verification](#release-verification).

### Build from source

macOS, Linux, and Windows are supported. The Rust version is pinned in
`rust-toolchain.toml` and installed automatically by `rustup`.

On Debian or Ubuntu, install the FLTK/X11 development packages first:

```bash
sudo apt-get install libx11-dev libxext-dev libxft-dev libxinerama-dev \
  libxcursor-dev libxrender-dev libxfixes-dev
```

The repository contains several binaries, so name the application one:

```bash
cargo run --release --bin space_query     # build and run
cargo build --release --bin space_query   # just the executable
```

The output is `target/release/space_query`, or `space_query.exe` on Windows.

## Getting started

**Connect.** **File > Connect** (`Ctrl+N`): pick a database type, fill in the
details, then test, save, or open it. Saved passwords go to the OS keyring, never
to the configuration file. For Oracle, choose Thin or OCI; Thin needs only Host,
Port, and Service.

![Database connection dialog](docs/images/connection-dialog.png)

**Write and run.** Type in the active query tab and use the toolbar or a
shortcut. On macOS, use `Cmd` where `Ctrl` is shown.

| Action | Shortcut |
| --- | --- |
| Execute the selection or the statement at the cursor | `Ctrl+Enter` or `F9` |
| Execute the whole script | `F5` |
| Quick describe the object at the cursor | `F4` |
| Open the definition of the object at the cursor | `Ctrl+B` |
| Search objects by name | `Ctrl+Shift+N` |
| Explain Plan / EXPLAIN | `F6` |
| Commit / roll back | `F7` / `F8` |
| Open completion | `Ctrl+Space` |
| Expand the code snippet at the cursor | `Tab` or `Ctrl+J` |
| Format the selection or current statement | `Ctrl+Shift+F` |
| Find in the editor, or in the result grid | `Ctrl+F` |
| Export the current result | `Ctrl+E` |

The complete list is under **Help > Keyboard Shortcuts**.

**Read the output.** The lower workspace keeps each kind of output apart: the
**Data Grid** holds query rows and plans, **Script Output** and **DBMS Output**
hold transcripts and server output, and **Messages** reports execution details,
affected-row counts, and errors.

**Finish.** **File > Disconnect** (`Ctrl+D`). Before disconnecting, switching
connections, committing, or rolling back, SPACE Query asks you to resolve any
running query, lazy fetch, open transaction, or pending grid edit that cannot be
closed safely.

## A few things it does differently

**Completion that knows your schema.** Suggestions come from the current SQL
context and the metadata already loaded for the connection — schemas, tables,
views, aliases, columns, routines, packages — and the same completion is
available in the table browser's `WHERE` and `ORDER BY` fields.

![SQL completion suggestions](docs/images/code-completion.png)

**Placeholders get answered, not rejected.** SQL copied out of application code
keeps its placeholders. `WHERE EMPNO = :id`, or the JDBC spelling `= ?`, opens a
prompt with one row per placeholder — name, type, value, and a `NULL` box —
instead of failing at the server. The value travels as its declared type, which
is what makes `FETCH FIRST :n ROWS ONLY` and `LIMIT :n` work. Oracle OUT
parameters and an OUT `SYS_REFCURSOR` are answered the same way.

![The bind-parameter prompt: one row per placeholder, each with a type, a value and a NULL box](docs/images/bind-parameters.png)

**A plan you can actually read.** On Oracle the plan is read out of `PLAN_TABLE`
rather than pre-rendered text, so every step keeps the optimizer's own values and
the tree is drawn from each step's real parent. `Cost %` is what a step spends on
*itself* — its cost minus its children's — which is what makes an expensive step
stand out from the ancestors that merely contain it.

![An Oracle execution plan drawn as a tree, with per-step cost share](docs/images/explain-plan.png)

**Read-only that means it.** A read-only connection classifies every statement
on its own: a script of three `SELECT`s runs, a `DELETE` hidden among them does
not, and anything the classifier cannot place is refused rather than allowed.
The controls that would start a write are removed instead of left to fail. It is
a guard against a slip, not a server-side lock — for that, use the connection's
**Access: Read only** transaction mode or an account without write privileges.

**Editing rows only when a row can be identified.** Oracle uses `ROWID`; MySQL
and MariaDB use a primary key or a non-null unique key. Changes stage in the
grid and reach the database on **Save**, under a one-row rule with the whole
save rolled back on conflict. Joins, multi-table results, and results without a
reliable identifier stay read-only rather than guessing.

**Color-tagged connections, worn where you look.** A tagged connection's color
rides on the query tab and on the result tabs beneath it, so the tab you are
about to run on is the one showing the color. A result keeps the color of the
connection that produced it, even after its tab is bound elsewhere.

![A red-tagged query tab whose result strip holds a green result from an earlier connection next to the selected red one](docs/images/connection-color-tabs.png)

**Nothing lost to a crash.** Unsaved editor tabs are snapshotted every few
seconds and the snapshot is deleted on a normal exit — so it only survives an
abnormal one, and finding it at startup is what prompts the offer to reopen.

## Oracle connection modes

| Capability | Thin | OCI (thick) |
| --- | --- | --- |
| External Oracle client | Not required | Instant Client or full client required |
| Address | Host / Port / Service | Host / Port / Service, or TNS alias |
| Transport | TCP | TCP or TCPS |

**OCI client discovery** looks at `ORACLE_CLIENT_LIB_DIR`, then `ORACLE_HOME`
(`%ORACLE_HOME%\bin` on Windows, `$ORACLE_HOME/lib` elsewhere), then an
`instantclient_*` directory under the platform defaults (`/opt/oracle` on macOS;
`/opt/oracle` or `/usr/local/oracle` on Linux; `C:\oracle` or
`%ProgramFiles%\Oracle` on Windows). If that fails, set the directory yourself:

```bash
export ORACLE_CLIENT_LIB_DIR=/opt/oracle/instantclient_23_3
```

On Apple Silicon the application and the client library must be the same CPU
architecture.

**TNS aliases** are OCI-only. Point `TNS_ADMIN` at the directory holding
`tnsnames.ora`:

```bash
export TNS_ADMIN=/opt/oracle/network/admin
```

Without it, Oracle Net falls back to `$ORACLE_HOME/network/admin`. Instant
Client has no equivalent default, so it normally requires `TNS_ADMIN`.

## Where your data lives

Standard OS roots, from the Rust `dirs` library:

| Data | Location |
| --- | --- |
| Settings, connection profiles, recent files | `config_dir()/space_query/config.json` |
| Query history | `data_dir()/space_query/query_history.json` |
| Application log | `data_dir()/space_query/app.log.json` |
| Crash report | `data_dir()/space_query/crash.log` |
| Unsaved editor tabs, kept only between an abnormal exit and the next start | `data_dir()/space_query/unsaved_tabs.json` |
| Saved passwords | the `space_query` service in the OS keyring |

Passwords are never written to `config.json`. Data left in the legacy
`oracle_query_tool` config and keyring namespaces is migrated when found.

## Development

```bash
cargo check --locked --bin space_query
cargo test --locked
cargo test --locked --manifest-path crates/tns-thin/Cargo.toml
```

`tns-thin` is a path dependency rather than a workspace member, so it has its
own test command. Oracle Thin live and comparison tests run through
`./test_tns_thin.sh` and need a configured local database.

Some behavior can only be proven by running the real UI or a real server, so it
lives in dedicated binaries — `verify_import_live`, `verify_explain_plan_live`,
`verify_bind_prompt_live`, `verify_value_edit_live`, `verify_read_only_live`,
and `verify_grid_features_live` take `all` to sweep every backend, while
`verify_import_ui`, `verify_bind_prompt_ui`, `verify_value_viewer_ui`,
`verify_column_layout_ui`, and `verify_editor_convenience_ui` drive the
application's own event loop:

```bash
cargo run --bin verify_grid_features_live all
cargo run --bin verify_column_layout_ui
```

Pull requests and pushes to `main` run formatting, Clippy, both non-live test
suites, and Linux x86_64 / macOS arm64 / Windows x86_64 build checks.

On macOS, the screenshots in `docs/images` are regenerated by
`./scripts/capture_feature_tour.sh`; pass a scene name to capture just one.

| Document | Purpose |
| --- | --- |
| [`docs/features.md`](docs/features.md) | Full user-facing feature reference |
| [`docs/oracle.md`](docs/oracle.md) · [`docs/mysql.md`](docs/mysql.md) · [`docs/mariadb.md`](docs/mariadb.md) | Per-backend development and live-test setup |
| [`docs/session.md`](docs/session.md) | Session ownership, cancellation, lazy fetch |
| [`docs/transaction.md`](docs/transaction.md) | Transaction and retained-session behavior |
| [`docs/result_ui.md`](docs/result_ui.md) | Result tabs and grid behavior |
| [`docs/formatting.md`](docs/formatting.md) · [`docs/highlighting.md`](docs/highlighting.md) | Formatter and highlighter internals |
| [`docs/new_backend.md`](docs/new_backend.md) | Checklist for adding a database backend |
| [`docs/README.md`](docs/README.md) | The rest of the developer documentation, in reading order |

## Release verification

Every release ships `SHA256SUMS` for its archives:

```bash
sha256sum --check SHA256SUMS   # macOS: shasum -a 256 --check SHA256SUMS
```

Archives also carry GitHub artifact provenance attestations:

```bash
gh attestation verify space_query-macos-arm64.zip --repo letspurify-ux/space_query
```

Checksums verify integrity; attestations verify the build origin. Neither is a
substitute for Apple Developer ID or Windows Authenticode signing, which these
archives do not have. Each archive contains the executable, `DISCLAIMER.md`, and
a `licenses/` directory with the SPACE Query licenses, third-party notices,
dependency licenses, `tns-thin` provenance, referenced upstream notices, and the
copyright text for the exact Rust toolchain that built the binary.

## License

SPACE Query's own code is offered under `MIT OR Apache-2.0`
([`LICENSE-MIT`](LICENSE-MIT), [`LICENSE-APACHE`](LICENSE-APACHE)). The software
is provided "as is", without warranty of any kind, and remains subject to
[`DISCLAIMER.md`](DISCLAIMER.md) — you are responsible for reviewing the
statements you run and for keeping backups.

The bundled `tns-thin` crate is licensed under Apache-2.0: parts of the Oracle
Thin implementation are modified works based on Apache-2.0 material from
`python-oracledb`, and parts were developed with reference to `go-ora`, whose
MIT license file covers only that material. It contains no Oracle client
software.

Release binaries statically link FLTK, which is distributed under the GNU
Library General Public License, Version 2, with the FLTK exceptions — including
its static-linking exception. SPACE Query is based in part on the work of the
FLTK project (<https://www.fltk.org>) and, through FLTK's image libraries, on the
work of the Independent JPEG Group. Other statically linked components include
ODPI-C (under the Universal Permissive License, Version 1.0 option), `cfltk`
(MIT), and Zstandard (BSD option).

Attribution, exact upstream revisions, and full dependency license texts are in
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md),
[`THIRD_PARTY_DEPENDENCIES.md`](THIRD_PARTY_DEPENDENCIES.md),
[`crates/tns-thin/THIRD_PARTY_NOTICES.md`](crates/tns-thin/THIRD_PARTY_NOTICES.md),
and [`crates/tns-thin/PROVENANCE.md`](crates/tns-thin/PROVENANCE.md).

### Trademarks

Oracle, Java, MySQL, SQL\*Plus, and NetSuite are trademarks or registered
trademarks of Oracle and/or its affiliates. MariaDB is a trademark of MariaDB
Corporation Ab. Other names may be trademarks of their respective owners. These
names are used here only to identify the software SPACE Query connects to or
builds on. This project is independent, and is not affiliated with, endorsed by,
or sponsored by Oracle, MariaDB Corporation Ab, or any other vendor.
