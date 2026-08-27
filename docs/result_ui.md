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

Only Data Grid results provide lazy fetch, selection/copy, result export,
supported grid editing, and individual tab close. `active_result_id()` returns
an ID only when both the Data Grid section and a real inner result are selected.
Editing, exporting, or closing a hidden grid while a support pane is selected
is forbidden by this rule.

Besides ordinary query results, ref cursors, Quick Describe, object-browser
queries, and Explain Plan can use Data Grid. `append_explain_plan_tab()` takes a
`QueryResult` built by `SqlEditorWidget::build_explain_plan_result`, which renders
`ExplainPlanData` through `explain_plan::plan_grid`. Oracle plans arrive as
`ExplainPlanData::Tree` — `PLAN_TABLE` rows with real parent links, drawn with
connector glyphs in the `Operation` column — and MySQL/MariaDB plans as
`ExplainPlanData::Flat`, which keeps the server's own `EXPLAIN` columns. Every
column is text, because the values are already formatted for reading.

A plan the SERVER draws itself is split one grid row per line
(`one_grid_row_per_plan_line`). MySQL's `EXPLAIN ANALYZE` and `FORMAT = TREE`,
and both products' `FORMAT = JSON`, answer with ONE column holding the whole
plan as a single string, newlines and all — as one cell the grid sized the row
to one line, so the user saw the first line of their plan and the rest lived
behind a double-click, while the same keystroke on MariaDB's tabular
`ANALYZE SELECT` produced a readable table. Only the one-column-with-newlines
shape is split, the server's column keeps its name, no column is invented, and
each line stays exactly as the server drew it, so grid search, selection and
export work on it like any other result. Live-gated by
`verify_a_server_drawn_plan_is_readable`, in a spelling each product really has.

F6 asks the splitter TWO things about that text, on both roads: which statement
it is, and whether it is a statement at all. Only the selection road used to ask
the second one, so a line the app runs ITSELF — `DESC t`, `CONNECT user/pass@db`,
`@script.sql` — was wrapped into an explain and SENT from the caret while the
identical text selected was refused and `Ctrl+Enter` answered it from the app's
own catalog. On the MySQL family the send even succeeded: `explain_plan_sql`
passes a `DESC` through, so the server answered with a TABLE DESCRIPTION under
the label "Explain Plan". `EXPLAIN ANALYZE <statement>` is unaffected — it is not
a tool command — and `DESC ANALYZE <statement>`, which is, is the same server
statement written the one way this app has never sent.

The splitter says which statement and where it ends; the user's text says what
the statement IS (`statement_with_its_leading_comments`), so routing the caret
road through it changes no bytes on the wire.

There is a THIRD question, and it is asked of the statement rather than of the
text: **has this statement an execution plan at all?** F6 wraps whatever it is
handed, so a PL/SQL block, a routine call, transaction or session control, and
this family's `ANALYZE … TABLE` maintenance statement were all wrapped and SENT
— four backends answering one keystroke with four different server complaints
(measured: Oracle 23c answers `ORA-00905` to `EXPLAIN PLAN FOR CALL p(1)`, a
block, `COMMIT` and `ANALYZE TABLE … COMPUTE STATISTICS`, and `ORA-00900` to
`ALTER SESSION SET …`; MySQL 8.0.46 answers `ERROR 1064` to `EXPLAIN CALL p(1)`
and `EXPLAIN DO …`). `ANALYZE TABLE t` was worse than an unhelpful error:
MySQL reads the wrapped `EXPLAIN ANALYZE TABLE t` as an executing explain of the
`TABLE t` QUERY, so F6 on a maintenance statement drew a real measured plan of a
full scan of that table, while MariaDB answered `ERROR 1064` and Oracle a parse
error. One keystroke, three different wrong answers.

`sql_classification::statement_without_an_execution_plan_reason` is the one
reader, and it is FAIL-OPEN by design: only what the app can PROVE has no plan
is named, so DDL as a class is deliberately absent — Oracle plans a
`CREATE TABLE … AS SELECT` and a `CREATE INDEX`, which share a `SqlKind` with
`CREATE PROCEDURE`. The reader consults the GRAMMAR before the kind, because
`SqlKind` is the session-safety taxonomy and answers the plan question only by
coincidence: a `SHOW` really returns rows, so it classifies `SelectLike` — the
one kind the gate trusted as certainly plannable — and every `SHOW` walked
through and was wrapped and SENT (measured: `ERROR 1064` on MySQL 8.0.46 and
MariaDB 12.2.2 for `EXPLAIN SHOW INDEX FROM t` and every other `SHOW`;
`ORA-00905` for `EXPLAIN PLAN … FOR SHOW PARAMETER …`, where `SHOW` is
SQL*Plus's word and begins no server statement at all). The grammar readers
(`statement_is_table_maintenance`, `statement_is_client_report_or_lock`) also
name the rest of the maintenance family (`CHECK`/`CHECKSUM`/`OPTIMIZE`/`REPAIR
… TABLE`, all measured `ERROR 1064` when wrapped, all previously leaking
through the kind match as fail-open `Ddl`), `HELP` (both MySQL products read
`EXPLAIN HELP 'x'` as a DESCRIBE of a table named `HELP` — the `ANALYZE TABLE`
shape again), and Oracle's `LOCK TABLE` (DML to the classifier, so fail-open;
`ORA-00905` measured). The MySQL family's `LOCK`/`UNLOCK` need no new name:
its classifier reads them as session control and the kind match already
refuses those. It is asked in the body of
`ExplainPlanBackend::refusal_before_sending`, the only default method that trait
has, so no backend can answer it differently or skip it; each backend still
answers its own half (`refusal_from_what_this_explain_does`). And it is asked of
`ExplainStatement::statement_the_app_chose_to_explain()`, never of the wire
text: where the user typed the explain themselves the app is not choosing and
has nothing to second-guess, which is why the MySQL family answers `None` for a
passthrough while Oracle always answers `Some` (it re-wraps even an
`EXPLAIN PLAN` the user typed, because the read-back needs the `STATEMENT_ID`
that write stamps).

What F6 explains is what `Ctrl+Enter` would send, and that is ONE decision:
`SqlEditorWidget::statement_source_for_single_action` — the selection when there
is one, otherwise the statement at the caret, normalized the same way. Both
`execute_statement_at_cursor` and `statement_to_explain` take it whole. They
used to decide separately, so F6 ignored the selection entirely and explained
whichever statement the caret sat in; two conditions for one question is also
how they came to disagree about the empty case (`selection_text().is_empty()`
versus `Fl_Text_Buffer::selected()`, which is true for a collapsed selection
carrying no text). A selection holding more than one statement is refused rather
than narrowed to its first — execution would run all of them, and picking one is
a guess (`single_statement_in_selection`). Placeholder values come from the same
prompt every other execution entry point uses, where they change what is sent
(`ExplainPlanBackend::prompts_for_placeholder_values`: the MySQL family
substitutes them into the text, while Oracle's `EXPLAIN PLAN` only parses the
statement it explains and needs none).

MariaDB's `SET STATEMENT <assignments> FOR <statement>` wrapper is one more
spelling only that product has, and the explain goes INSIDE it: `EXPLAIN SET
STATEMENT … FOR SELECT …` — what wrapping the whole wrapper produced — is
`ERROR 1064` (measured, 12.2.2), while `SET STATEMENT … FOR EXPLAIN SELECT …`
answers with a real plan. The classification side always unwrapped the wrapper
(`SqlStatementAnalysis::new_for_db_type`), so the builder and the
executing-explain reader reading only the leading `SET` was one text with two
readings; `mariadb_set_statement_wrapper(db_type, sql)` now hands the builder
BOTH halves from one parse, the builder rebuilds `SET STATEMENT <assignments>
FOR EXPLAIN <inner>` (the assignments are KEPT — they can change the very plan
being asked about), an inner that is already an explain passes the whole
wrapper through, and `mysql_explain_executed_statement` sees through the
wrapper so a wrapped `ANALYZE <write>` is refused exactly as the bare spelling
is — and so the implicit-commit and uncommitted-work tables judge what
actually runs. MySQL never takes this road: the wrapper reader takes the
`db_type`, and there the words are a syntax error the gate judges as the `SET`
they lead with.

Every one of those decisions reads ONE prepared view of the text —
`sql_classification::statement_reader_view`, the same preparation the
classifier has always used: leading comments stripped and, on the MySQL
family, executable comments (`/*! … */`, `/*M! … */`) EXPANDED, because those
servers execute their content. Parsed raw instead, `/*! ANALYZE */` was a
comment to the readers and an ANALYZE to the server — measured: MySQL 8.0.46
ran `EXPLAIN /*! ANALYZE */ SELECT SLEEP(2)` for the full two seconds, MariaDB
12.2.2 ran `/*! ANALYZE */ UPDATE …` and wrote — so
`EXPLAIN /*! ANALYZE */ UPDATE …` walked through the write gate that exists to
refuse an executing explain of a write (on MySQL ≥ 8.3 the iterator executor
runs the UPDATE). The pass-through wire still carries the user's raw bytes;
only the DECISIONS read the expanded view, and Oracle's view expands nothing
because its servers give those comments no meaning.

MariaDB's `SHOW EXPLAIN FOR <id>` / `SHOW ANALYZE FOR <id>` — its own explains
of a RUNNING statement — pass through as the user's explain (measured: real
plan rows, `ERROR 1094 Unknown thread id` for a bad id), where MySQL keeps the
SHOW refusal because there the spelling is `EXPLAIN FOR CONNECTION <id>`
(1064 for the SHOW forms, measured). One reader
(`mariadb_show_spelled_explain`) answers both the refusal gate and the
builder, so the one cannot refuse what the other passes through.

Both roads are driven against a real server by
`verify_explain_plan_live`, whose two statements read different tables so the
plan itself says which one was explained.

Everything F6 shows — the plan tab, the note, a refusal, a failure — is
delivered by the WORKER, on the operation's own progress channel, BEFORE
`OperationFinished` (`deliver_explain_plan_outcome`): the same contract every
other result keeps. It used to ride a second channel to a UI-side poll that
re-sent it under the operation's token a beat AFTER the worker had finished the
operation — so the moment anything newer owned the tab (F6 auto-repeat, F6 then
`Ctrl+Enter` inside the poll's 50 ms window) the delivery filter dropped a plan
that had really come back, in silence. The `UiActionResult` now carries only
the status-bar line, so the late road is unwritable.

The status line and the Messages pane say what the result said: a plan with no
steps reports `No plan output.` rather than claiming it loaded, and a cancelled
F6 reports `result_messages::EXPLAIN_PLAN_CANCELLED` on every backend instead of
`ORA-01013` on Oracle and `Query execution was interrupted` on the MySQL family.

A REFUSAL is not a FAILURE, and `ExplainPlanError` carries the difference. Both
used to be a bare `String`, so the pane prefixed every one of them and a refusal
read `Explain plan failed: Explain plan was not run: …` — the app announcing a
failure of its own rule. `Refused` reaches the pane as its own sentence,
`Failed` is evidence and gets the prefix, a cancel can only hide in `Failed`
(a refusal never travelled to a server to be cancelled), and the note that says
what a plan cannot see goes with `Failed` alone: it opens "This plan was built
on the connection's own DB session", which is a fact about a plan that was
attempted.

Both refusals name their subject in the words of the product that answered.
`explain_plan_would_run_the_statement` takes the SPELLING out of the same answer
that decided the statement is an executing explain
(`ExecutingExplainWrite::spelling`), because MySQL writes
`EXPLAIN ANALYZE <statement>` and MariaDB — which rejects that spelling outright
— writes `ANALYZE <statement>`; a hardcoded `EXPLAIN ANALYZE` told a MariaDB
user their statement was refused for being a form of explain their server does
not have and they had not typed, and the guard test asserted that on both
products. A read-only refusal goes through
`explain_plan_write_refused`, which keeps the shared read-only wording verbatim
and adds the sentence only the backend can
(`ExplainPlanBackend::why_building_the_plan_is_itself_a_write`): on Oracle the
plan itself is the write, which is why F6 is refused there for a `SELECT` and
simply works on the other family. And a tab bound to no connection is told that,
not that the connection is busy.

A TIMEOUT is not a cancel, and telling them apart is the server's wording
against the app's: measured on MySQL 8.0.46, `max_execution_time` reports
`ERROR 3024 (HY000): Query execution was interrupted, maximum statement
execution time exceeded` — which opens with the exact sentence `KILL QUERY`
produces, so a timed-out F6 announced itself as the user's own cancel and threw
the timeout away. `session_policy::message_indicates_query_cancel` now asks the
app's own cancel sentence first, a timeout report second (its markers are
measured per backend), and the driver's cancel markers last — the same
precedence the driver-level readers have always applied. The note that says what
a plan cannot see goes with a failure but never with a cancel: nothing about
what the tab's session holds explains a plan the user stopped.

The grid asks the wider question — was this statement ABORTED, cancelled or
timed out? — before deciding whether to keep the rows a SELECT already
streamed. Both leave the same thing behind: real rows the server sent and the
user is looking at. It used to ask about the cancel alone, which reached a
MySQL timeout only because that server words one like a `KILL QUERY`, and never
reached Oracle's `DPI-1067` or the app's own timeout sentence — so the same
event kept the partial grid on one backend and replaced it with an error row on
another (`an_aborted_select_keeps_its_rows_whichever_backend_ended_it`).

## Selection totals

`selection_summary.rs` aggregates the selected cells for the status bar. The
rules it holds to:

- Only the grid actually on screen reports a summary
  (`ResultTabs::selection_summary_label` gates on `is_on_screen()`), and only
  when the selection covers more than one cell.
- SQL aggregate semantics: NULLs are skipped and `Count` is the number of
  non-NULL values, not the number of selected cells. The zero-width auto-`ROWID`
  column of edit mode is never aggregated.
- Sums are exact decimal arithmetic over the driver's own spelling (`i128` at a
  common scale), never `f64`. If a value is not a plain decimal, or the exact
  arithmetic leaves `i128` range, the numeric part is dropped and only `Count`
  is reported — an approximate total is never shown.
- The status bar asks on every animation frame, so the scan is memoized on
  (selection bounds, data generation, row count) and a selection above
  `MAX_SCANNED_CELLS` reports its size without scanning.

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

**A save never opens a transaction over work of the user's.** It has to be
atomic and it must not resolve what is not its own, and those two meet in one
place: under auto-commit the only way to be atomic is to open a transaction, and
MySQL's `START TRANSACTION` implicitly COMMITS whatever the session already
holds. An auto-commit tab CAN hold one — an explicit `START TRANSACTION` survives
auto-commit ON, and the app supports that deliberately — so bracketing the save
by the tab's auto-commit flag committed the user's uncommitted work for them,
unrecoverably, and reported only the save's own success. The question is "is
there anything of the user's to lose", answered once for every backend by
`db::app_operation_transaction_scope()`: with nothing open the save owns its
transaction and commits it; otherwise it nests in a `SAVEPOINT` and leaves the
decision to the user, exactly as the manual-commit path always did. Oracle's save
meets the same rule by construction — its block opens with `SAVEPOINT` and has no
transaction-opening statement to reach for. The save's own message follows the
scope it really used, so a nested save never claims the rows are committed.

**A ROWID save is refused once the tab has moved.** The Oracle path names the
table exactly as the user's `SELECT` did, and the save runs later through the
tab's own execute path, which asserts whatever scope the tab has THEN — so an
unqualified name plus a schema change in between resolves against the NEW schema,
and a staged INSERT lands in a same-named table there and reports success.
Qualifying the name instead would change how it resolves (a public synonym stops
resolving once a schema is put in front of it), so the save says no. The
comparison is total: "no scope" is a VALUE — the login schema, or the
connection's own database — not a missing answer, because the one door that
installs a result's rows records the tab's scope as it does so. Reading it as
"nothing to disagree with" let the same wrong-schema write through whenever the
tab moved INTO or OUT OF a scope rather than between two. The connection,
generation, pool epoch, database type and a scope picked in the object browser
are answered before that by the result's own `ExecutionOrigin`; what reaches this
check is a scope the user's own script moved.

## Cell value window

`src/ui/value_viewer.rs`. Opened by double-clicking a cell, or by
`View Value` / `Edit Value` in the Data Grid menu; the entry point is
`ResultTableWidget::open_cell_value_window`, which targets the top-left cell of
the selection and skips the hidden auto-`ROWID` column.

A read-only value gets a `TextDisplay`, an editable one a `TextEditor`. FLTK has
no read-only flag on the editor, and the display-only widget already selects,
scrolls, and copies. Editability is decided by `cell_value_is_editable`, the
same test the inline editor applies, and it is asked **again** after the window
closes — a save can start while the window is open.

A saved value goes through `apply_cell_edit_value`, shared with the inline
editor, so the NULL decision, dirty-cell bookkeeping, and repaint cannot drift
between the two ways of editing a cell.

`Format` is a view, never an edit. The indented text lives only in the buffer
while the box is ticked; the text being edited waits in `raw_text` and is what
`Save` writes. Both formatters move whitespace and nothing else: `format_json`
re-spaces a validated token list rather than round-tripping through a document
model (which would reorder keys and reformat numbers), and `format_xml` indents
only elements whose content is entirely other elements, because whitespace
inside mixed content is content.

## Long values in Oracle grid edits

The MySQL family saves through binds, so length is not a concern there. Oracle
renders values as SQL literals, which has two limits:

- A string literal over 4000 bytes is `ORA-01704`.
  `ResultTableWidget::oracle_text_literal` emits
  `TO_CLOB('..') || TO_CLOB('..')` past a safe threshold. Chunks are cut from
  the unescaped value, so a boundary can never land inside an escaped `''` pair
  or mid-character. Below the threshold the literal is byte-for-byte what it
  always was.
- `clob_column = 'text'` is `ORA-22848` at any length, so a table with a `CLOB`
  column could not be edited at all. `original_value_predicate` emits
  `DBMS_LOB.COMPARE(col, ..) = 0` for character LOBs and `col = ..` for
  everything else. The LOB columns are identified from the declared types the
  driver reported (`column_data_types`), not guessed from value length.

`src/bin/verify_value_edit_live.rs` drives both statement shapes against every
backend with a multi-byte value containing quotes and newlines, and compares the
value read back byte for byte.

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

`resolve_export_table` names the table from the grid-edit descriptor when there
is one, otherwise from the SQL that produced the grid. That SQL is
`ResultTableWidget::source_sql_snapshot`, which reports the finished statement
when there is one and the streaming statement otherwise: a grid that is still
fetching, and one a cancelled lazy fetch left populated, both keep their real
table name — the cancelled lazy fetch never sends a `StatementFinished` at all.
`QueryProgress::SelectStart` therefore carries `sql` alongside the column kinds.
Edit mode deliberately keeps reading the finished `source_sql`, so it still
cannot be entered mid-statement.

`SQL Inserts` and `Where Clause` are immediate. `SQL Updates` first reads the
table's primary key through `ObjectBrowserWidget::load_primary_key_columns`, so
it completes asynchronously via `FileActionResult::CopyToClipboard`.

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

## Result export

> Implementation: `src/ui/result_export.rs` (serializers),
> `src/ui/result_export_dialog.rs` (modal), `src/ui/result_table.rs`,
> `src/ui/main_window.rs`

`Ctrl+E`, **Tools > Export Results**, and the Data Grid popup's
**Export Results** all open one modal that asks three things:

| Choice | Values |
| --- | --- |
| Format | CSV, TSV, JSON, XML, HTML, Markdown, SQL Inserts |
| Rows | All rows, Selected rows (disabled without a selection) |
| Destination | File, Clipboard |

`SQL Inserts` only appears while a connection is active: its literals depend on
the dialect, and `grid_sql_export::build_sql_inserts` renders it — the serializers
in `result_export.rs` deliberately produce nothing for that format so a mis-routed
call yields nothing rather than wrong SQL. Everything else is dialect-neutral.

Column scope differs by format on purpose. The data formats export what the grid
shows and drop only the hidden auto-`ROWID` column, because the file should match
the screen. `SQL Inserts` keeps the stricter internal-column rule above, because
generated SQL needs legal column names.

NULL follows each format's own vocabulary: CSV and TSV write the grid's NULL
display text verbatim (a spreadsheet dump of what is on screen), JSON writes
`null`, XML writes an empty element, and HTML and Markdown write an empty cell.

The UTF-8 byte-order mark belongs to a *file*, not to the text.
`ExportFormat::file_byte_order_mark` returns it for CSV and TSV only, and only
the file destination prepends it: Excel decides a delimited file's encoding from
it, while pasting `U+FEFF` into an editor just inserts an invisible character.
`render` never emits one.
JSON leaves a value unquoted only when the driver typed the column `Number` or
`Boolean` *and* its text is already a valid JSON literal, so a zero-padded
`00123` and a leading-dot `.5` both stay quoted strings while `1.2E+10` — which
the JSON grammar does accept — stays a number.

An **All rows** export first completes any open lazy fetch — the render is
deferred into `LazyFetchPendingAction::Export` and runs when the fetch lands.
A **Selected rows** export never waits, because a selection can only cover rows
already on screen. Files are written off the main thread and reported through
`FileActionResult::Export`; the clipboard path queues
`FileActionResult::CopyToClipboard` so the copy happens on the main thread.

### Characters the markup formats cannot carry

XML 1.0's `Char` production excludes every C0 control except tab, newline, and
carriage return, and unlike `<` they cannot be rescued by a character reference
either — a `CHAR` column holding one would produce a document no parser accepts.
`escape_markup` replaces them (and U+FFFE/U+FFFF) with U+FFFD in both XML and
HTML, so the substitution is visible rather than a silently shortened value.
Carriage return is written as `&#13;`: a literal CR is folded into a newline by
the XML line-end rules and by the HTML5 input-stream preprocessor, and only a
character reference, which is resolved after that folding, survives.

Known limits, none of which the other formats share:

- A duplicate column name produces duplicate JSON keys and duplicate XML
  elements. Both are legal; a JSON parser keeps only the last occurrence.
- A JSON number is emitted unquoted, so a consumer that parses numbers as IEEE
  doubles loses precision on an Oracle `NUMBER` past 15–17 digits. The text
  itself is exact.
- A column name XML cannot start an element with (blank, or leading digit)
  becomes `column_<n>`; other illegal characters become `_`, so two different
  names can collapse onto the same element name.

### Verification

`cargo run --bin verify_result_export` renders one deliberately hostile grid —
commas, tabs, quotes, embedded newlines, a bare CR, `|`, `\`, `]]>`, a C0
control, Korean text, NULL, empty string, a zero-padded number, and column names
that are blank, duplicated, punctuated, or digit-leading — in every format and
hands the bytes to real parsers, comparing each cell back to what was written:

| Format | Validator |
| --- | --- |
| JSON | `serde_json`, numeric-aware cell comparison |
| XML | `xmllint --noout` for well-formedness, Python `ElementTree` for cells |
| HTML | Python `html.parser` |
| CSV / TSV | Python `csv`, `excel` and `excel-tab` dialects |
| Markdown | cell-count and unescape round-trip |

macOS's bundled HTML Tidy is deliberately unused: it dates from 2006, predates
HTML5, and misreads UTF-8, so it rejects `<!DOCTYPE html>`, `<meta charset>`,
and every Korean character. `SQL Inserts` is covered instead by
`verify_grid_sql_export` and `verify_grid_sql_export_live`.

`cargo run --bin verify_result_export_ui` covers everything between the keystroke
and that render, which no unit test reaches: it builds the real `MainWindow` with
its real callbacks, starts the export from the application's own menu bar, and
drives the production modal from a timeout inside the modal's own event loop —
setting the format, the scope, and the destination, then clicking Export. The
clipboard destination is checked byte for byte with `pbpaste` for every format,
along with both scopes, the withheld `SQL Inserts` entry, and Cancel writing
nothing. The file destination stops at the macOS save panel, which no in-process
code can drive; everything before it is the same code the clipboard path runs.

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
cargo test result_export --lib
cargo test --test ui_dialog_guards

# Every export format through real JSON/XML/HTML/CSV parsers, no database needed.
cargo run --bin verify_result_export

# The real window, the real menu, and the real modal, end to end to the
# clipboard. No database needed.
cargo run --bin verify_result_export_ui

# Real widget + real OS clipboard, no database needed.
cargo run --bin verify_grid_sql_export

# The cell value window: read-only shape, Format round trip, Save/Cancel.
cargo run --bin verify_value_viewer_ui

# Live: a long CLOB/TEXT value edited and read back, every backend.
cargo run --bin verify_value_edit_live all

# Live: real driver types and generated SQL executed on the server.
# Run one Docker container at a time (MySQL and MariaDB both bind 3306).
cargo run --bin verify_grid_sql_export_live thin
cargo run --bin verify_grid_sql_export_live oci
cargo run --bin verify_grid_sql_export_live mysql
cargo run --bin verify_grid_sql_export_live mariadb
```
