# Result UI Design

## Goal

Restructure the editor result area to follow a Toad-like layout: purpose-specific
top-level tabs, with nested tabs only where they help organize a single result
area. The design keeps query result grids as the source of statement lifecycle
state, while output, messages, DBMS output, and explain plans are routed to
dedicated supporting views.

## Toad-Like Top-Level Tabs

Use these fixed top-level tabs, in this order:

1. Data Grid
2. Script Output
3. DBMS Output
4. Messages
5. Explain Plan

Do not add Trace, Query Viewer, debugger, profiler, or Team Coding tabs in v1.
Those are Toad features, but they are outside this app's current feature set.

Do not add a separate Stats top-level tab in v1. Execution summaries should be
shown in the status bar or in Messages.

## Tab Responsibilities

### Data Grid

Data Grid is the primary destination for tabular results:

- SELECT result sets
- DML RETURNING result sets
- REF CURSOR results
- Quick Describe and object-browser tabular result views, unless a future design
  gives them a dedicated destination

Data Grid owns the existing result lifecycle:

- Running
- Fetching
- Waiting
- Canceling
- Done
- Error
- Cancelled

Nested tabs:

- Grid 1
- Grid 2
- Grid 3

The first v1 label can stay simple as `Grid N`. If status and row count are
useful in practice, extend labels to `Grid N - Done (128)` later, but avoid
making the first refactor depend on label polish.

Only Data Grid tabs support:

- lazy fetch state
- result editing
- copy/copy with headers
- paste into editable grids
- select all
- CSV export
- result tab close

### Script Output

Script Output is for SQL*Plus-style script transcript and command output.

Nested tabs:

- Output
- Errors
- Grids
- Environment

Routing:

- PROMPT output goes to `Script Output > Output`.
- ECHO output goes to `Script Output > Output`.
- SQL*Plus/tool command feedback goes to `Script Output > Output`.
- Script-level errors may be mirrored to `Script Output > Errors`, but SQL
  execution errors must also go to `Messages > Errors`.
- `Script Output > Grids` shows a lightweight list or summary of generated grid
  tabs, not duplicate result data.
- `Script Output > Environment` shows script variables/settings only when the app
  already tracks them. Do not invent new environment tracking in this refactor.

Keep the existing `append_script_output_lines(...)` API, but make its destination
`Script Output > Output`.

### DBMS Output

DBMS Output is for Oracle server output, primarily `DBMS_OUTPUT.PUT_LINE`.

Routing:

- Oracle DBMS output goes to `DBMS Output`.
- Do not mix DBMS output into Script Output unless the same line is part of an
  explicit script transcript feature.
- If DBMS output is the only result from an execution, select `DBMS Output`.
- If a Data Grid result is also produced, keep focus on Data Grid and just update
  DBMS Output in the background.

Add:

- `append_dbms_output_lines(lines)`

### Messages

Messages is the common information, warning, and error area.

Nested tabs:

- Info
- Errors
- Warnings

Routing:

- General execution information goes to `Messages > Info`.
- SQL execution errors go to `Messages > Errors`.
- Connection failures go to `Messages > Errors`.
- Driver warnings and database warnings go to `Messages > Warnings` when they are
  available as warnings.

Important invariant:

- SQL failure must not be moved out of Data Grid.
- The related `Data Grid > Grid N` must still be updated to Error so the
  statement lifecycle, close behavior, lazy fetch mapping, and progress context
  remain correct.
- Messages receives a mirrored, aggregate view of the error.

Add:

- `append_message_lines(kind, lines)`
- `select_messages_errors()`

### Explain Plan

Explain Plan is the dedicated destination for estimated/actual plan output.

Routing:

- Successful explain plan actions go to `Explain Plan`.
- Failed explain plan actions go to `Messages > Errors`.
- Do not create a normal Data Grid result tab for explain plan output in v1.

Add:

- `set_explain_plan_text(text)`
- `select_explain_plan()`

## Selection Rules

Default selection rules:

- SELECT/tabular result: select `Data Grid > Grid N`.
- Script transcript only: select `Script Output > Output`.
- SQL error: update `Data Grid > Grid N`, mirror to `Messages > Errors`, then
  select `Messages > Errors`.
- Connection failure: append to `Messages > Errors`, then select
  `Messages > Errors`.
- DBMS output only: select `DBMS Output`.
- DBMS output plus grid result: keep `Data Grid > Grid N` selected.
- Explain success: select `Explain Plan`.
- Explain failure: select `Messages > Errors`.

Do not auto-select Messages for ordinary informational messages if a Data Grid
result is available. Data Grid should stay the user's visual anchor for normal
query execution.

## Close And Editing Rules

Only `Data Grid > Grid N` can be closed.

When the active top-level tab is not Data Grid:

- Close should be disabled or no-op.
- Edit controls should be hidden or disabled.
- Export should be disabled or report that no data grid is active.
- Copy/select-all should apply to a grid only when a grid is active.

`active_result_index()` must return:

- `Some(index)` only when `Data Grid > Grid N` is selected.
- `None` for Script Output, DBMS Output, Messages, or Explain Plan.

This keeps existing result-grid edit/export code from accidentally operating on
stale or hidden grid state.

## Implementation Notes

Primary implementation area:

- `src/ui/result_tabs.rs`

Main routing updates:

- `src/ui/main_window.rs`
- `src/ui/sql_editor/mod.rs`

Recommended `ResultTabsWidget` public methods:

- Keep `append_script_output_lines(lines)`.
- Add `append_dbms_output_lines(lines)`.
- Add `append_message_lines(kind, lines)`.
- Add `set_explain_plan_text(text)`.
- Add `select_data_grid(index)`.
- Add `select_script_output()`.
- Add `select_messages_errors()`.
- Add `select_explain_plan()`.

Recommended message kind enum:

```rust
pub(crate) enum ResultMessageKind {
    Info,
    Error,
    Warning,
}
```

Avoid changing `QueryResult`, database executors, SQL parser wire types, or
session policy types for this UI refactor.

## Test Plan

Run:

```sh
cargo test result_tabs
cargo test main_window
cargo check
```

Expected coverage:

- Top-level tab order is fixed.
- Multiple result sets create `Data Grid > Grid N` nested tabs.
- `active_result_index()` returns `None` outside Data Grid.
- SQL failures update the grid state and mirror to `Messages > Errors`.
- Connection failures route to `Messages > Errors`.
- Script output routes to `Script Output > Output`.
- DBMS output routes to `DBMS Output`.
- Explain success routes to `Explain Plan`.
- Grid edit/export controls are active only for an active Data Grid tab.

## Out Of Scope For V1

- Trace tab
- Query Viewer tab
- PL/SQL debugger tabs
- Profiler tabs
- Team Coding tab
- Script Output history persistence
- Click-to-jump from errors to editor line/column
- Full environment/session variable tracking beyond what already exists
