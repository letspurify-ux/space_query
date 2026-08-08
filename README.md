# SPACE Query

SPACE Query is a desktop SQL client for Oracle, MySQL, and MariaDB, built with
Rust and FLTK. It brings connection management, object-aware SQL editing,
script execution, result inspection, and session diagnostics into one app.

Oracle needs no Instant Client: Thin mode speaks the protocol directly, so one
extracted binary is enough to connect, run a script, and read the result.

![SPACE Query main window](docs/images/main-window.png)

## Why SPACE Query

- **One app, three databases.** Oracle, MySQL, and MariaDB share the same
  editor, object browser, result grid, and diagnostics, while the SQL dialect,
  metadata queries, and transaction rules follow the active connection.
- **No Oracle client to install.** Thin mode connects over TCP with Host, Port,
  and Service. OCI stays available when TCPS or a TNS alias is needed.
- **Results you can work with.** Sort, page, and lazily fetch rows, edit a
  single-table result back to the server under a safe row identifier, and copy
  any selection as CSV or as SQL that runs unchanged.
- **Nothing happens behind your back.** Session activity, query history, and an
  application log show what each connection is doing, and disconnecting,
  committing, or switching connections stops for anything still unresolved.
- **A native desktop binary.** No runtime, no browser, no background service.

## At a glance

| Area | What you get |
| --- | --- |
| Editor | IntelliSense over live metadata, signature hints, code snippets, SQL-aware formatter, multiple file tabs, find and replace, quick describe |
| Execution | One statement, a selection, or a full script with SQL*Plus-style commands, bind variables, ref cursors, and per-statement timeouts |
| Results | Independent result tabs, lazy fetch, sorting, in-grid text search, selection totals, export to CSV/TSV/JSON/XML/HTML/Markdown/SQL, and staged in-grid editing |
| Objects | Filterable object tree, structure/index/constraint inspection, DDL generation, confirmed drop/truncate, file import into a table, and table browsing with database-side paging |
| Operations | Session activity, persisted query history, application log, and crash reports |

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

- For Oracle, choose Thin or OCI. Thin uses Host, Port, and Service without an
  external client.
- For MySQL or MariaDB, enter Host, Port, Username, Password, and an optional
  database, then adjust SSL or session options if necessary.

![Database connection dialog](docs/images/connection-dialog.png)

### 2. Edit and execute SQL

Write SQL in the active query tab, then use the toolbar or a shortcut. On
macOS, use `Cmd` where `Ctrl` is shown.

| Action | Shortcut |
| --- | --- |
| Execute the selection or statement at the cursor | `Ctrl+Enter` or `F9` |
| Execute the complete script | `F5` |
| Quick describe the object at the cursor | `F4` |
| Open the definition of the object at the cursor | `Ctrl+B` |
| Search objects by name | `Ctrl+Shift+N` |
| Explain Plan / EXPLAIN | `F6` |
| Commit / roll back | `F7` / `F8` |
| Open IntelliSense | `Ctrl+Space` |
| Expand the code snippet at the cursor | `Tab` or `Ctrl+J` |
| Format the selection or current statement | `Ctrl+Shift+F` |
| Go to a line number | `Ctrl+G` |

The complete shortcut list is available under **Help > Keyboard Shortcuts**.

### 3. Inspect results

The lower workspace keeps each output type separate:

- **Data Grid** shows query rows and Explain Plan / EXPLAIN results, with
  selection, copy, data and SQL export, sorting, and lazy-fetch controls.
- **Script Output** and **DBMS Output** retain script transcripts and server
  output.
- **Messages** reports execution details, affected-row counts, and errors.

Use **File > Disconnect** (`Ctrl+D`) when finished. Before disconnecting,
switching connections, committing, or rolling back, SPACE Query asks you to
resolve any running query, lazy fetch, transaction, or pending grid edit that
cannot be closed safely.

## Features

### Connection colors and read-only mode

Every saved connection carries two client-side settings, next to its name and
host rather than under Advanced Settings, because neither is a session option
and neither is sent to the server.

![The connection dialog with a color picked and Read-only ticked](docs/images/connection-color.png)

**Color** tags the connection so the window says which database is on the other
end before a statement runs. The tag colors the status bar's connection dot and
the label of every query tab bound to that connection. A dropped connection
always shows the disconnected color instead, so a tag can never be mistaken for
a live session.

**Read-only** refuses to send anything that writes. Each statement is classified
on its own — a script of three `SELECT`s runs, a `DELETE` hidden among them does
not — and anything that is not provably a read is refused, including a statement
the classifier cannot place. `SELECT`, `WITH`, `DESCRIBE`, `SHOW`, session
settings such as `USE` and `ALTER SESSION SET CURRENT_SCHEMA`, and
`COMMIT`/`ROLLBACK` all still run. `INSERT`, `UPDATE`, `DELETE`, `MERGE`, DDL,
PL/SQL blocks, and procedure calls do not. A `@file` include is refused because
its contents cannot be checked first, and a SQL\*Plus `CONNECT` because it would
leave the read-only connection behind.

Two statements that look like reads are refused because they are not:
`SELECT ... FOR UPDATE` takes row locks that outlive it, and Oracle's
`EXPLAIN PLAN FOR` inserts rows into `PLAN_TABLE` — so `F6` Explain Plan is
unavailable on a read-only Oracle connection. MySQL and MariaDB's `EXPLAIN` only
reports and stays available.

The controls that would start a write are removed rather than left to fail: the
grid's **Edit** checkbox does not appear, and the object browser drops
**Drop**, **Truncate**, **Import Data**, and **Execute Procedure/Function** from
its menus. Reading the catalog — **Generate DDL**, **View Structure**,
**Check Compilation** — is unaffected. The status bar reads
`name (Oracle) · read-only`.

This is a guard inside SPACE Query, not a server-side lock. It protects against
a slip, not against a determined attempt. For a server-enforced restriction, use
the connection's **Access: Read only** transaction mode or a database account
without write privileges.

### SQL editor and IntelliSense

IntelliSense uses the current SQL context and loaded database metadata to
suggest keywords, schemas, tables, views, aliases, columns, routines, packages,
and other objects. Use the arrow keys to select an item, `Enter` or `Tab` to
insert it, and `Esc` to close the popup.

![SQL IntelliSense suggestions](docs/images/intellisense.png)

When the cursor is inside a function or procedure call, the signature hint
shows the available parameters and emphasizes the active argument. It follows
typing and mouse cursor movement, and closes when the application window moves
or resizes so it cannot remain detached from the editor.

![Function signature hint with the active argument emphasized](docs/images/signature-popup.png)

The editor also supports multiple SQL file tabs, open/save/recent files,
syntax highlighting, find and replace, undo/redo, comment toggling, selection
case conversion, SQL block selection, execution timeouts, and previous/next
query-history navigation.

#### Code snippets (live templates)

Type an abbreviation and press `Tab` to expand it into a statement skeleton.
The first placeholder is selected, so typing replaces it; `Tab` again moves to
the next one, and `Esc` leaves the template. Placeholders are found again by the
literal text around them, so a name typed over the first placeholder does not
throw off the ones after it.

![The `sel` abbreviation expanded, with its first placeholder selected](docs/images/code-snippets.png)

`Ctrl+J` (`Cmd+J` on macOS) expands the abbreviation too, and works while the
completion popup is open — with the popup up, `Tab` still inserts the selected
suggestion, as it always did. Pressing `Ctrl+J` where there is no abbreviation
before the cursor opens the list of what can be typed, which is also under
**Help > Code Snippets**.

![The built-in code snippets and their bodies](docs/images/snippet-reference.png)

The built-in templates cover `SELECT`, `SELECT COUNT(*)`, `INSERT`, `UPDATE`,
`DELETE`, inner and left joins, `CASE`, `CREATE TABLE`, and the PL/SQL block,
`IF`, and numeric `FOR` loop. A multi-line body is indented to match the line
the abbreviation was typed on.

#### Soft wrap and Go to Line

**Edit > Soft Wrap** wraps long lines at the editor's right edge instead of
scrolling sideways, which is what generated DDL and long `IN (...)` lists need.
The setting is saved, so it applies to every tab — the ones already open, the
ones opened afterwards, and the ones restored on the next start. The line-number
gutter keeps numbering buffer lines, not wrapped rows.

![A long statement wrapped at the editor's right edge](docs/images/soft-wrap.png)

`Ctrl+G` asks for a line number and puts the caret there — the shortest path
from a script error that names a line to the line itself. A number past the end
of the buffer goes to the last line rather than being refused; anything that is
not a plain number is reported instead of guessed at.

#### Go to declaration and object search

`Ctrl+B` resolves the object name under the caret and opens its source in a new
editor tab: the body of a view, procedure, function or package, and the `CREATE`
statement of a table. A package member opens its package, which is where its
source lives. Resolution is the object browser's own, so `Ctrl+B` opens exactly
what the tree would open for the same name — schema-qualified names and package
members included.

`Ctrl+Shift+N` searches the objects of the current scope by name. Exact matches
come first, then prefixes, then anything containing the text. The arrow keys
move through the results while the caret stays in the search box, and `Enter`
opens the highlighted object's source. The list is built from the metadata the object
browser has already loaded, so it answers without a round trip and never shows
something the tree does not.

![Searching objects by name, with each match labelled by kind](docs/images/object-search.png)

### SQL formatting

The formatter applies SQL-aware line breaks and indentation in place. It uses
the Oracle or MySQL/MariaDB dialect path of the active connection.

| Before | After `Ctrl+Shift+F` |
| --- | --- |
| ![SQL before automatic formatting](docs/images/sql-formatting-before.png) | ![SQL after automatic formatting](docs/images/sql-formatting-after.png) |

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

#### Bind parameter values

SQL copied out of application code keeps its placeholders. Running
`SELECT * FROM EMP WHERE EMPNO = :id` — or the JDBC spelling,
`... WHERE EMPNO = ?` — opens a prompt for the values instead of sending a
statement the server cannot answer.

![The bind-parameter prompt: one row per placeholder, each with a type, a value and a NULL box](docs/images/bind-parameters.png)

Every placeholder gets a row: its name, a type, the value, and a `NULL` box that
disables the value field. The type matters in the places where a quoted string
is not merely a different value but a syntax error — Oracle
`FETCH FIRST :n ROWS ONLY` and MySQL `LIMIT :n` need a number, not `'2'`. `Date`
and `Timestamp` values are written as `YYYY-MM-DD HH:MM:SS` on every backend. An
empty box means SQL NULL for every type but `String`, since there is no such
thing as an empty number or an empty date.

PL/SQL OUT parameters are answered the same way: leave the value empty and pick
the type. Oracle adds a `Ref Cursor` type for an OUT `SYS_REFCURSOR`, which
disables the value and `NULL` controls on that row — so
`BEGIN emps_by_dept(:dept, :cnt, :rc); END;` runs and shows the cursor's rows
without a `VARIABLE` line in front of it. The values the call assigns come back
into the session and are reported the way `VARIABLE` binds always were.

Every way of calling a routine is covered, for procedures and functions alike:

| | Procedures | Functions |
| --- | --- | --- |
| Oracle | `BEGIN p(:a, :b); END;` · `EXEC` · `EXECUTE` · `CALL` · `DECLARE … BEGIN … END;` | `SELECT f(:a) FROM DUAL` · `BEGIN :r := f(:a); END;` · `EXEC :r := f(:a)` |
| MySQL / MariaDB | `CALL p(:a, @out)` | `SELECT f(:a)` |

`EXEC` is worth a note: it is rewritten into a PL/SQL block deep in the
execution worker, long after the prompt reads the text you wrote, so the
placeholders are recognized in the spelling you typed. On MySQL an OUT argument
must be a user variable — `@out` is not a placeholder, so it passes through
untouched while the IN value beside it is substituted.

What reaches the server differs by family, because the two do not offer the same
thing. On Oracle the values become real binds of the declared type and the
statement text is sent exactly as written. MySQL and MariaDB have no bind path
here, so the placeholders are replaced by properly quoted literals before the
statement is sent — which is also the SQL the query history records, since it is
what actually ran. Oracle `?` placeholders are rewritten to generated bind names
(`:SQ_P1`, …), because the server does not accept `?` at all.

Only placeholders with no value are asked about. A bind declared with
`VARIABLE` is a standing declaration and is never prompted for, so
`VARIABLE id NUMBER` + `EXEC :id := 7369` keeps working untouched, and a
statement mixing a declared bind with an undeclared one asks only about the
latter. A prompted value is *not* a declaration: it is offered again, prefilled,
on the next run, so changing it takes one keystroke rather than a new
declaration. Cancelling the prompt runs nothing at all.

The answer is carried to the server as its declared type, not as text pasted
into the statement, so it matches the column it is compared against whatever
that column is — `NUMBER` / `NUMBER(p,s)` / `BINARY_DOUBLE`, `VARCHAR2` /
`CHAR` / `NVARCHAR2`, `DATE` / `TIMESTAMP` / `TIMESTAMP WITH TIME ZONE`, `CLOB`
and `RAW` on Oracle; `INT` / `BIGINT` / `DECIMAL` / `DOUBLE`, `VARCHAR` /
`CHAR` / `TEXT`, `DATE` / `DATETIME` / `TIMESTAMP` / `TIME`, `BLOB` and `JSON`
on MySQL and MariaDB. Non-ASCII text survives the round trip.

Colons that are not placeholders are left alone — a `'HH24:MI:SS'` format model,
a `q'[a:b]'` literal, a `:=` assignment, and the `:NEW` / `:OLD` correlation
names in a `CREATE TRIGGER` body.

### Explain plan

`F6` explains the statement at the cursor and shows the plan in its own Data
Grid tab.

![An Oracle execution plan drawn as a tree, with per-step cost share](docs/images/explain-plan.png)

On Oracle the plan is read out of `PLAN_TABLE`, not out of pre-rendered
`DBMS_XPLAN` text, so every step keeps the values the optimizer produced. The
connectors in the `Operation` column are drawn from each step's real parent, so
the shape on screen is the one the database reported — nothing is inferred from
indentation. `Cost` is the cumulative cost Oracle reports; `Cost %` is what the
step spends *on itself* — its cost minus its children's — as a share of the
plan's total, which is what makes an expensive step stand out from the
ancestors that merely contain it. Access and filter predicates sit on the rows
they belong to instead of in a footnote.

MySQL and MariaDB keep the server's own `EXPLAIN` columns — `id`, `select_type`,
`table`, `type`, `key`, `rows`, `Extra` and the rest — with a `Rows %` share
added. No tree is drawn there: a classic `EXPLAIN` has no parent column, and
deriving one from `id` and `select_type` would be a guess rather than a report.

Because the plan is an ordinary grid, everything the grid does works on it:
`Ctrl+F` to find a step, selection totals, copy, and export.

### Object browser

The filterable object tree supports refresh, data queries, structure/index/
constraint inspection, DDL generation, and package routine browsing. Oracle
groups tables, views, procedures, functions, sequences, triggers, synonyms,
and packages. MySQL/MariaDB groups tables, views, procedures, functions,
triggers, and events, and shows sequences when the server exposes them.

![Object browser with example Oracle metadata](docs/images/object-browser.png)

#### Drop and truncate

The context menu ends with **Truncate...** on tables and **Drop...** on the
object types each backend can drop by name — tables, views, procedures,
functions, sequences and triggers on both families, materialized views,
synonyms and packages on Oracle, and events on MySQL/MariaDB. Indexes are not
offered: `DROP INDEX` needs the table the index belongs to, which a tree node
does not carry.

Neither runs on the click. A dialog first shows the exact statement that would
be sent and asks for confirmation, and only then is that statement put in the
editor and executed like any other — leaving you holding the SQL that ran. The
statement is the plain one you read: no `CASCADE CONSTRAINTS`, no `PURGE`, and
nothing else widened on your behalf. A drop that the database refuses reports
its own error rather than being retried with a broader statement.

![Confirming a drop with the statement it would run](docs/images/object-drop-confirmation.png)

### Table browsing and bounded paging

Double-click a table in the object browser to open its rows in a dedicated
result tab. The filter bar above the grid keeps the **WHERE** and **ORDER BY**
editors at equal widths as the window resizes. Enter expressions without the
`WHERE` or `ORDER BY` keywords, press `Enter` to apply them, or use the clear
button to reset a field.

![Table browser with WHERE, ORDER BY, and paging controls](docs/images/table-browse.png)

The same metadata-aware IntelliSense used by the SQL editor is available in
both fields. Start typing or press `Ctrl+Space` (`Cmd+Space` on macOS); the
suggestion popup opens directly below the text cursor in the active field. It
moves above the field or stays within the screen edge when space is limited.

![WHERE IntelliSense positioned at the text cursor](docs/images/table-browse-popup.png)

![ORDER BY IntelliSense listing the table's columns](docs/images/table-browse-order-popup.png)

Use the first, previous, next, and last controls below the grid and choose a
page size of 10, 100, 250, 500, or 1,000 rows. Table browsing issues a bounded
query for each page—Oracle uses `ROWNUM`, while MySQL and MariaDB use
`LIMIT/OFFSET`—and does not retain a lazy-fetch cursor between pages. Completed
table tabs show `Page N` with the current page row count, and row headers retain
their absolute positions across pages.

### Result grid

- Drag or use the keyboard to select cells; `Ctrl+C` copies the selection and
  `Ctrl+Shift+C` includes headers.
- Resize columns, sort by a column header, or export the result with `Ctrl+E`.
- Configure the maximum cell preview length and lazy-fetch batch size. Scrolling
  near the end fetches more rows, while full-result actions can fetch all
  remaining rows first.
- Use the context menu to close a result, copy data as text or SQL, export data,
  or access available edit actions.

![Result grid with a multi-cell selection](docs/images/result-grid.png)

#### Find text in the rows on screen

With the result grid focused, `Ctrl+F` (`Cmd+F` on macOS) searches the rows that
have already been fetched. Nothing is sent to the server and no statement is
re-run, so this stays available on a result the filter bar cannot re-query.

Every matching cell is highlighted and the current match is picked out in a
brighter shade; the counter shows where you are in the result. `Enter` and
**Next** step forward, `Shift+Enter` and **Previous** step back, and both wrap
around. A search starts from the selected cell rather than from the first row,
and matching is case-insensitive unless **Case sensitive** is ticked. The
highlight is cleared when the dialog closes.

Matching covers the full stored value, not the shortened text a narrow cell
draws, and it uses the same search the editor's **Find** uses.

![Find in Results highlighting every matching cell](docs/images/grid-search.png)

#### View and edit a cell value

A grid draws each cell as one clipped line, which is no use for a CLOB, a JSON
document, or any long text. Double-click a cell, or pick **View Value** from the
Data Grid context menu, to open it in a window with the whole value, soft
wrapped, plus its length in characters and bytes.

When the result is in **Edit** mode and the cell is one edit mode can write, the
same window opens as an editor and the menu entry reads **Edit Value**. Saving
stages the new value exactly as the inline editor would, so it reaches the
database on the next **Save** and nowhere earlier.

**Format** shows an indented copy of a JSON or XML value. It is a view and only
a view: clearing it returns the exact bytes you were editing, and saving always
writes those bytes, never the indented version. Formatting moves whitespace and
nothing else — JSON keeps its key order and its numbers as written, and an XML
element containing text is left alone, because in XML that whitespace is
content. The box is disabled for a value that is neither.

![The value window showing an indented JSON CLOB](docs/images/value-viewer.png)

Long values now save on every backend. MySQL and MariaDB always could — their
saves use binds — while Oracle rendered values as SQL literals, so a value over
4000 bytes was `ORA-01704` and a `CLOB` column could be read but never written.
Oracle values past that size are now written as concatenated `TO_CLOB` chunks,
and the original-value guard compares character LOBs with `DBMS_LOB.COMPARE`,
which is the comparison Oracle accepts (`clob_column = 'text'` is `ORA-22848` at
any length).

#### Selection totals in the status bar

Selecting more than one cell puts a count, sum, average, minimum, and maximum
for that selection at the right end of the status bar. Nothing is sent to the
server — it is the data the grid already holds — and the line disappears again
as soon as the selection is a single cell.

![Selection totals for a numeric column in the status bar](docs/images/selection-summary.png)

The aggregate follows SQL semantics: NULLs are skipped, and **Count** is the
number of non-NULL values in the selection rather than the number of cells.
Sums are exact decimal arithmetic on the value the driver produced, so
`0.1 + 0.2` is `0.3` and a long Oracle `NUMBER` keeps every digit. When any
selected value is not a plain number — text, a date, a number with thousands
separators — only **Count** is shown, because there is nothing meaningful to
total. A selection too large to scan reports its size instead.

For a safely identifiable Oracle, MySQL, or MariaDB single-table result,
**Edit** mode can stage inserted, updated, deleted, or `NULL` values. Oracle
uses `ROWID`; MySQL and MariaDB use a primary key or a non-null unique key.
Changes reach the database only after **Save**. MySQL/MariaDB locks each
existing target and checks its original values before mutation; Oracle uses
guarded `ROWID` DML with the same one-row rule. The whole save is rolled back
on a conflict. JOINs, multi-table results, and results without a reliable row
identifier remain read-only.

![Oracle result grid in staged edit mode](docs/images/result-grid-editing.png)

### Export a result

`Ctrl+E`, **Tools > Export Results**, and the Data Grid context menu's
**Export Results** all open the same dialog: pick a format, pick whether to
export every row or just the selection, and pick a file or the clipboard.

![The Export Results dialog: format, row scope, and destination](docs/images/result-export.png)

| Format | Notes |
| --- | --- |
| **CSV**, **TSV** | Files start with a UTF-8 BOM so Excel reads non-ASCII text correctly; the clipboard gets none |
| **JSON** | Array of objects; SQL `NULL` becomes `null`, and only genuinely numeric text stays unquoted |
| **XML** | `<results><row>…`; illegal characters in a column name become `_` |
| **HTML** | A standalone document with a plain bordered table |
| **Markdown** | Pipe table; `\|` is escaped and line breaks become `<br>` |
| **SQL Inserts** | The same statements the context menu copies, for the whole result and straight to a file |

**SQL Inserts** needs a live connection, because its literals follow the
connected dialect; the other formats do not care. Exporting every row finishes an
open lazy fetch first, so the file has the whole result rather than the rows
scrolled into view. Exporting a selection never triggers a fetch.

### Import a file into a table

Right-click a table in the object browser and choose **Import Data...**. Every
format the export writes can be read back, so a result exported from one
database goes straight into another.

![The Import Data dialog: format, header and NULL choices, and the column mapping](docs/images/table-import.png)

The dialog re-reads the file whenever a choice changes, so the column list, the
row count, and the mapping on screen always describe what **Import** would
actually run.

| Choice | What it does |
| --- | --- |
| **Format** | Preselected from the file's extension; change it to read the same file another way |
| **First row is a header** | CSV, TSV, and HTML only. On, the file names its columns; off, they are `COLUMN_1…n` |
| **NULL text** | CSV and TSV only. A cell holding exactly this text becomes SQL `NULL`. Defaults to `NULL`, which is what the export writes |
| **File column → table column** | One selector per file column, preset by matching names (or by position when there is no header). Send a column to `(skip)` to leave it out |

Each format is read back the way it was written, so an export/import round trip
returns the original values:

| Format | `NULL` is |
| --- | --- |
| **CSV**, **TSV** | A cell whose text equals the NULL text; a UTF-8 BOM is stripped |
| **JSON** | The `null` literal. Numbers keep their exact spelling, and a nested object or array is kept verbatim |
| **XML** | An empty element written `<C/>`; `<C></C>` is the empty string |
| **HTML**, **Markdown** | An empty cell |
| **SQL Inserts** | The `NULL` keyword. `TO_DATE`, `TO_TIMESTAMP`, `TO_TIMESTAMP_TZ`, and `HEXTORAW` are unwrapped back to the value they wrap, and any other expression is refused by name rather than quoted into something else |

Literals are built from the **target column's declared type**, not from how a
value looks, exactly as the SQL export does — so a zero-padded code keeps its
quotes and its zeros, and a date-shaped string going into a `VARCHAR2` stays a
string. Rows are batched (100 per statement: a multi-row `VALUES` list on
MySQL and MariaDB, `INSERT ALL` on Oracle) and run as an ordinary script, so the
import commits exactly when the session's auto-commit setting says it does and a
failure is reported like any other statement's.

Two things the import will not do quietly: a file that is not UTF-8 is refused
rather than mangled, and an `&` in a value is written as `CHR(38)` on Oracle so
it cannot be read as a substitution variable.

Formats that cannot express a value exactly are the documented edges: Markdown
trims a cell and writes every line break as `<br>`, HTML and Markdown spell
`NULL` and the empty string the same way, and a CSV cell holding the NULL text
is indistinguishable from `NULL`.

### Copy a selection as SQL

Select cells in the Data Grid, right-click, and take the selection as SQL on the
clipboard, ready to paste and run:

| Menu item | Clipboard contents |
| --- | --- |
| **SQL Inserts** | `INSERT INTO <table> (<selected columns>) VALUES (…);` per selected row |
| **SQL Updates** | `UPDATE <table> SET <selected non-key columns> WHERE <primary key>;` per row |
| **Where Clause** | `AND` within a row, `OR` between rows, and `IN` when one column is selected |

![The Data Grid menu over a selection, with the SQL it produces pasted in the editor](docs/images/grid-sql-export.png)

Values are rendered from the column types the driver reported, not from how a
value happens to look, so a `NUMBER` stays bare, a `DATE` becomes `TO_DATE(…)`
on Oracle or a quoted ISO string on MySQL/MariaDB, and a zero-padded `CHAR` code
such as `00123` keeps its quotes and its zeros. Oracle `RAW` becomes
`HEXTORAW('…')`.

```sql
-- Data Grid > SQL Inserts, for two rows of EMPNO, ENAME, HIREDATE
INSERT INTO EMP (EMPNO, ENAME, HIREDATE) VALUES (7369, 'SMITH', TO_DATE('1980-12-17','YYYY-MM-DD'));
INSERT INTO EMP (EMPNO, ENAME, HIREDATE) VALUES (7499, 'ALLEN', TO_DATE('1981-02-20','YYYY-MM-DD'));

-- Data Grid > Where Clause, for the EMPNO column alone
EMPNO IN (7369, 7499)
```

Only what you selected is exported; helper columns the grid uses internally
never appear. **SQL Updates** identifies rows by the table's real primary key,
read from the database, and takes the key values from the whole row, so a key
column outside the selection still identifies its row—if the table has no usable
key, the `WHERE` clause is omitted and the status bar says so. Results whose base
table cannot be determined, such as a join, fall back to `MY_TABLE`. A result
that is still fetching, or one a cancelled fetch left on screen, exports under
its real table name. See [docs/result_ui.md](docs/result_ui.md) for the per-type
rules and their known limits.

### Settings and diagnostics

**Settings > Preferences** controls editor/result fonts, global UI size, result
preview and lazy-fetch limits, connection-pool size, cancellation behavior, and
how many query-history and application-log entries are retained. Settings
persist between launches.

![Application settings](docs/images/settings.png)

**Tools > Query History** searches executed statements, filters failures, shows
SQL and error details, and sends a selected statement back to the editor.
History is kept in a file and restored at the next launch; **Settings >
Preferences > History & Log** sets how many query-history and application-log
entries are retained.

![Query history with SQL and error preview](docs/images/query-history.png)

**Tools > Session Activity** shows active and retained sessions, running SQL,
lazy fetches, result-tab ownership, fetched-row counts, and elapsed time.

![Session activity result](docs/images/session-activity.png)

**Tools > Application Log** filters, inspects, exports, and clears log entries.
If the app panics, it records `crash.log` and displays that report at the next
launch.

![Application log viewer](docs/images/application-log.png)

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

Some behaviour can only be proven by running the application or a server. Those
checks are separate binaries:

```bash
# The import modal, driven through its own event loop.
cargo run --bin verify_import_ui

# Soft wrap, Go to Line and the object search modal, driven through the
# application's own menu bar.
cargo run --bin verify_editor_convenience_ui

# Export a table, import the file back, compare — every format, every backend.
cargo run --bin verify_import_live all

# Explain plan shape and Go to Declaration — every backend.
cargo run --bin verify_explain_plan_live all

# The bind-parameter modal, driven through its own event loop.
cargo run --bin verify_bind_prompt_ui

# Bind parameter values, declared and prompted — every backend.
cargo run --bin verify_bind_prompt_live all

# The cell value window, driven through its own event loop.
cargo run --bin verify_value_viewer_ui

# A long CLOB/TEXT value edited and read back — every backend.
cargo run --bin verify_value_edit_live all

# A read-only connection refuses writes and the database is unchanged, with a
# writable control group proving each statement is a real write — every backend.
cargo run --bin verify_read_only_live all
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
