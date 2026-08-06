# Result UI Structure

> Implementation: `src/ui/result_tabs.rs`, `src/ui/main_window.rs`,
> `src/ui/result_table.rs`

The result area separates statement-scoped tabular results from accumulated
support output.

## Top-level sections

`ResultTabsWidget::top_level_tab_labels()` creates four tabs in this fixed order:

1. Data Grid
2. Script Output
3. DBMS Output
4. Messages

Explain Plan is a result tab inside Data Grid, not another top-level section.

## Data Grid

Each statement result has a `ResultTabId` and one `ResultTabStatus`:

```text
Running, Fetching, Waiting, Canceling, Done, Error, Cancelled
```

Only Data Grid results provide lazy fetch, selection/copy, CSV export,
supported grid editing, and individual tab close. `active_result_id()` returns
an ID only when both the Data Grid section and a real inner result are selected.
Editing, exporting, or closing a hidden grid while a support pane is selected
is forbidden by this rule.

Besides ordinary query results, ref cursors, Quick Describe, object-browser
queries, and Explain Plan can use Data Grid. `append_explain_plan_tab()` converts
plan text to a one-column `QueryResult` whose column is named `Text`.

## Grid editing

`src/db/result_edit.rs` defines the backend-neutral edit descriptor, typed row
snapshot, mutation request, and exact-row execution rules. An editable result
must identify one base table and every existing row uniquely:

- Oracle uses `ROWID`.
- MySQL and MariaDB prefer the primary key, then a complete non-null unique
  index. Nullable, functional, or incomplete unique indexes are not locators.
- JOINs, CTEs, derived tables, and other ambiguous result shapes stay read-only.
  Computed or duplicate source columns are not editable.

MySQL/MariaDB save pre-locks and verifies affected rows against their original
typed values. Oracle uses guarded `ROWID` DML and verifies `SQL%ROWCOUNT`.
Every update or delete is constrained to one row. A stale row, missing row,
duplicate locator, cancellation, or execution error rolls back the whole save
(or only its savepoint inside an existing manual transaction), leaving the
staged grid changes available for correction or retry.

## Grid SQL export

> Implementation: `src/ui/grid_sql_export.rs`, `src/ui/result_table.rs`,
> `src/ui/main_window.rs`

The Data Grid popup menu turns the selected cells into SQL on the clipboard,
matching DataGrip's `SQL Inserts`, `SQL Updates`, and `Where Clause` extractors:

| Item | Output |
| --- | --- |
| SQL Inserts | `INSERT INTO <table> (<selected columns>) VALUES (…);` per selected row |
| SQL Updates | `UPDATE <table> SET <non-key columns> WHERE <primary key>;` per row |
| Where Clause | AND within a row, OR between rows, `IN` for a single column |

Only the selection is exported. Internal columns never appear: the hidden
auto-`ROWID` column, a visible `ROWID`, `SQ_INTERNAL_EDIT_SNAPSHOT`,
`SQ_INTERNAL_EDIT_KEY_*`, and blank names (which is what `SET HEADING OFF`
produces). `SQL Updates` reads key values from the whole row, so a primary-key
column outside the selection still identifies the row; with no usable key the
WHERE clause is omitted and the status bar says so. An unresolvable base table
renders as `MY_TABLE`.

`SQL Inserts` and `Where Clause` are immediate. `SQL Updates` first reads the
table's primary key through `ObjectBrowserWidget::load_primary_key_columns`, so
it completes asynchronously via `FileActionResult::CopySqlToClipboard`.

### Literals come from driver types, never value shapes

Each backend classifies its own column-type enum into `SqlValueKind`
(`src/db/query/types.rs`), carried to the grid on
`QueryProgress::SelectStart { column_kinds }`. A kind list that disagrees with
the header count is discarded rather than zipped, so a mismatch degrades to
quoted strings instead of mislabelling a column.

| Kind | Oracle | MySQL / MariaDB |
| --- | --- | --- |
| `Number`, `Boolean` | bare value | bare value |
| `Temporal` | `TO_DATE` / `TO_TIMESTAMP` / `TO_TIMESTAMP_TZ` by rendered shape | quoted ISO text |
| `Binary` | `HEXTORAW('…')` | quoted text (see below) |
| `String`, `Unknown` | quoted, `''` escaped | quoted, `''` and `\\` escaped |

Because the kind comes from metadata, a `VARCHAR2` holding `2024-01-01` stays a
string and a zero-padded `00123` keeps its zeros.

Known limits, all pre-existing display behaviour rather than export bugs:

- MySQL/MariaDB binary values reach the grid through
  `String::from_utf8_lossy`, so they cannot round-trip. `BINARY`/`VARBINARY`
  also arrive as `VAR_STRING`; only `BLOB` classifies as `Binary`.
- Oracle OCI renders LOBs as `[LOB]`; the thin driver carries real content.
- A string whose text is `"NULL"` is indistinguishable from SQL NULL.
- Oracle rejects string literals over 4000 bytes (32767 with extended string
  size). Long values are emitted verbatim, never truncated.
- Client-built grids (`PRINT`, `SHOW ERRORS`, `COMPILE ERRORS`, …) have no driver
  types, so every value is quoted — correct, since they are text tables.

## Support panes

| Section | Inner structure | Input API |
| --- | --- | --- |
| Script Output | Output, Errors | `append_script_output_lines()` appends to Output |
| DBMS Output | One text pane | `append_dbms_output_lines()` |
| Messages | Info, Errors | `append_message_lines(ResultMessageKind, ...)` |

`ResultMessageKind` contains only `Info` and `Error`; there is no warning kind.
The Script Output Errors pane exists in the UI but currently has no dedicated
append route. Execution errors go to Messages Errors and to the associated Data
Grid status.

When Script Output exceeds `SCRIPT_OUTPUT_MAX_CHARS`, its prefix is trimmed on a
line boundary toward `SCRIPT_OUTPUT_TRIM_TARGET_CHARS`.

## Selection policy

- Starting a tabular result selects its Data Grid result.
- A support pane may be selected when it is the only output and the current
  operation has no selected grid.
- Error messages select Messages Errors.
- Informational output does not steal focus from a visible Data Grid.
- A successful Explain Plan selects its new Data Grid result.

Actual routing uses the operation token and progress context in
`main_window.rs`, including `should_select_support_result_pane()`.

## Close and clear

- Users close individual Data Grid results.
- Closing a result returns any attached lazy-fetch session ID to cleanup.
- Support sections are cleared rather than closed.
- `clear_current_support_section()` empties buffers belonging to the selected
  support section.
- A full clear removes every Data Grid result and support buffer.

`ResultTabCloseTarget::ScriptOutput` and the script-output close methods remain
as compatibility surface, but those close methods currently return `false`.

## Verification

```sh
cargo test result_tabs --lib
cargo test result_table --lib
cargo test main_window --lib
cargo test grid_sql_export --lib
cargo test --test ui_dialog_guards

# Real widget + real OS clipboard, no database needed.
cargo run --bin verify_grid_sql_export

# Live: real driver types and generated SQL executed on the server.
# Run one Docker container at a time (MySQL and MariaDB both bind 3306).
cargo run --bin verify_grid_sql_export_live thin
cargo run --bin verify_grid_sql_export_live oci
cargo run --bin verify_grid_sql_export_live mysql
cargo run --bin verify_grid_sql_export_live mariadb
```
