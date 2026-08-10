# Transactions and Retained Sessions

> Implementation: `src/db/transaction.rs`, `src/db/session_policy.rs`,
> `src/db/connection.rs`

A retained session is a physical database session kept by a query tab for a
later action. Its state is not a single dirty flag: transaction, session
residue, and locks are tracked independently.

## State model

`RetainedSessionState` has three components.

| Component | Tracked state |
| --- | --- |
| `TransactionSessionState` | `Clean`, `MaybeDirty`, `BlockedDirty`, `DecisionRequired`, `InvalidSession` |
| `SessionResidueState` | Temporary table, prepared statement, user variable, transaction-mode override, untracked session state |
| `SessionLockState` | Table, flush-table, backup, and named locks |

`MaybeDirty` means a known transaction can continue on the same session; it
does not inherently require user resolution. `BlockedDirty` and
`DecisionRequired` block ordinary execution until resolved.

## Capabilities drive UI actions

`RetainedSessionState::capabilities()` calculates:

- Whether commit or rollback is available
- Whether the physical session may be discarded
- Whether residue/locks require discard after transaction resolution
- Whether transaction options may change
- Whether ordinary execution is blocked

Do not infer buttons or preflight behavior from the transaction enum alone. A
residue-only or lock-only session may require explicit cleanup or
`DiscardPhysical`, not commit or rollback.

## State after a statement

`src/db/sql_classification.rs` classifies SQL as `SqlKind`.
`statement_session_post_processor_for()` produces database-specific
`StatementSessionEffects`, and `retained_session_state_after_statement()` merges
those effects with the previous retained state.

The important rules are:

- Successful plain `COMMIT` and `ROLLBACK` clear transaction state.
- DML, transaction control, and DDL implicit commit use database-specific
  semantics.
- Temporary objects, prepared statements, variables, and locks survive as
  state separate from the transaction.
- PL/SQL, `CALL`, and unknown session effects are recorded conservatively.
- A successful health check does not prove a clean transaction.

A MySQL/MariaDB one-shot `SET TRANSACTION` override stays on the same physical
session until the next transaction-starting statement consumes it. Oracle
transaction mode follows the first-statement constraint.

## Tab-scoped transaction mode

> Implementation: `SqlEditorWidget::tab_transaction_mode_override`
> (`src/ui/sql_editor/mod.rs`), worker resolution and per-backend threading in
> `src/ui/sql_editor/execution.rs`, toolbar handling in `src/ui/main_window.rs`

Transaction mode (isolation level + read/write access mode) is a query-tab
setting, like auto-commit:

- `DatabaseConnection::transaction_mode` is only the connection default,
  seeded from the connection's advanced settings at connect. A new tab has no
  override and follows it.
- The toolbar isolation/access choices show and pin the ACTIVE tab's effective
  mode (`effective_transaction_mode(connection_default, tab_override)`).
  Changing them touches nothing else — not the shared connection, not other
  tabs, not the pool-context epoch.
- Execution resolves the effective mode once at worker startup
  (`transaction_mode_for_execution`) and threads it through every backend
  path: Oracle OCI/thin apply it per execution, and the MySQL/MariaDB pooled
  acquire, the pre-action scope recheck, and grid-edit saves all override
  `DbPoolSessionContext::transaction_mode` with the tab's value.
- The tab pin survives an in-script `CONNECT`, resolved over the new
  connection's default.

### Query-driven changes mirror into the tab and the UI

`session_transaction_mode_change_for_statement()` (`src/db/transaction.rs`)
recognizes SESSION-persistent changes only: MySQL/MariaDB
`SET SESSION|LOCAL TRANSACTION ...` and session-scoped
`transaction_isolation`/`tx_isolation`/`transaction_read_only`/`tx_read_only`
assignments; Oracle `ALTER SESSION SET ISOLATION_LEVEL = ...`. When such a
statement succeeds, every batch loop adopts it: the tab override is pinned to
the merged mode and `QueryProgress::TransactionModeChanged` re-syncs the
toolbar immediately. Because the tab setting now represents the session
truthfully, the session-scope transaction-mode override residue is dropped
(`with_session_transaction_mode_override_adopted`) so the tab's next
execution is not blocked.

One-shot `SET TRANSACTION ...` forms, unqualified `@@` assignments,
GLOBAL/PERSIST scopes, and unrecognized values are NOT adopted; they keep the
conservative override-residue tracking described above.

### Oracle: returning to Default resets the session

`ALTER SESSION SET ISOLATION_LEVEL` is SESSION persistent, and Oracle's
statement list for the default mode is empty — so selecting "Default" again
after such a statement would leave the session on the abandoned level while the
toolbar reads the connection default. Both Oracle drivers therefore resolve
their statements through
`DatabaseConnection::oracle_transaction_mode_statements_for_tab()`, which
prepends `ALTER SESSION SET ISOLATION_LEVEL = <connection default>` whenever the
tab has actively selected the default isolation. A tab that never touched the
controls has adopted nothing and pays nothing. The reset's effects are
deliberately not recorded as session residue: it restores a state the tab
already represents, so it must not make the next execution stop for a
resolution decision.

### Oracle: Read only is enforced in the client on both drivers

Oracle expresses read-only as a property of the TRANSACTION
(`SET TRANSACTION READ ONLY`), so a `COMMIT` inside the user's own batch ends it
and every statement after it would run read-write. Both Oracle batch loops
therefore refuse non-queries client-side
(`oracle_read_only_allows_statement()`) while the tab's access mode is Read
only; the server's ORA-01456 is only the backstop. MySQL/MariaDB need no such
gate — `SET SESSION TRANSACTION READ ONLY` survives the commit by itself.

### Unrunnable isolation/access pairs are refused at selection

Isolation and access mode are independent choices, so a user can select a pair
a backend has no statement for (Oracle cannot combine READ ONLY with an
explicit isolation level). `update_transaction_mode_from_controls()` checks
`DatabaseConnection::transaction_mode_selection_error()` and refuses the pair
where it is chosen, instead of pinning a mode that makes every later statement
fail.

### Screen = session guarantee

The toolbar sync records the mode it displayed
(`record_displayed_transaction_mode`), and execution startup refuses to run
when its own resolution disagrees (`transaction_mode_display_mismatch_error`,
self-healing) — one checkpoint before backend dispatch covers all four
backends. The choices are disabled whenever the active tab cannot accept a
mode change right now (running query/lazy fetch, or a retained state that
requires resolution). The guard test
`transaction_mode_state_has_a_single_source_of_truth`
(`tests/concurrency_multithread_guards.rs`) pins the resolver chain, the
display cross-check, and the adoption wiring.

### MySQL/MariaDB: end the residual transaction before applying mode

Under `autocommit=0` the app's own bookkeeping statements (dirty probes,
session-setting validation) leave an implicit transaction open on a pooled
session, and MySQL fixes a transaction's isolation and access mode at
transaction START. Without ending that residual transaction, a
`SET SESSION TRANSACTION ... READ ONLY` only takes effect one transaction
later (live-observed on MySQL 8.0: a READ ONLY pin let the next INSERT
through and blocked the one after). `mysql_pooled_execution_session_setup_statements`
therefore issues `ROLLBACK` first — safe because setup statements only run
when the retained state carries no user work.

## Central preflight

Every execution, setting change, and connection-lifecycle action is checked as
a `RetainedSessionPreflightAction`:

- `Execute`
- `TransactionOptionChange`
- `ScopeChange`
- `ConnectionTransition`
- `PoolResize`
- `Close`
- `ReleaseClean`
- `Discard`

The result is `Allow` or `RequireResolution`. An execution with SQL text uses
`retained_session_state_execute_preflight_decision_for_sql()`. Even in a blocked
state, it may allow the one statement proven to clean the relevant lock/residue
or consume a pending transaction-mode override.

## User resolution

`RetainedSessionResolutionAction` has three variants:

- `Commit`
- `Rollback`
- `DiscardPhysical`

Call `ensure_retained_session_resolution_action_allowed()` before acting. Even
after successful commit or rollback, `discard_after_transaction_resolution` may
require discarding the session because residue or locks remain. Closing a tab,
switching connections, or resizing the pool must never silently commit or roll
back user work.

## Ownership and identity

- A query tab owns its retained lease storage.
- Commit and rollback bind to the selected tab and lease at request time.
- A lease with a different connection generation, pool-context epoch, or
  database type is not reusable.
- Changing current schema/database never lowers retained state to clean.
- Retained-lease conflicts use `conservative_merge()` and the central conflict
  policy.

For the effect of cancellation and timeout on this state, see the
[session lifecycle](session.md).

## Verification

```sh
cargo test transaction --lib
cargo test retained_session --lib
cargo test session_policy --lib
cargo test --test concurrency_multithread_guards

# Live, one Docker database at a time:
cargo run --bin verify_transaction_mode_live -- <thin|oci|mysql|mariadb|all>
cargo run --bin verify_auto_commit_live -- <thin|oci|mysql|mariadb|all>
```

`verify_transaction_mode_live` drives the real editor per backend: a READ
ONLY tab pin blocks writes while the connection default stays untouched, a
session-scoped statement adopts into the tab and emits
`TransactionModeChanged`, the adopted mode is really applied on the next
execution, and one-shot `SET TRANSACTION` does not repin the tab. It also
covers the scope and session claims that only a server can settle: a READ ONLY
pin still refuses the write after the batch's own `COMMIT` (S10), returning the
tab to Default really puts the session back — read behaviourally on Oracle with
a second session, from the session variable on MySQL/MariaDB (S11), a second
tab on the same connection is unaffected by the pin (S12), and an unrunnable
isolation/access pair is reported where it is selected (S13).

`verify_auto_commit_live` covers the tab-scoped auto-commit model on the same
four backends: the connection default really commits, the menu write path pins
only the active tab, a second tab on the same connection stays manual, the
dirty guard refuses a change and leaves it without effect, a Read only tab
still refuses the write with auto-commit on, and (Oracle) a read-only
transaction is not ended by a piggybacked wire commit.

When adding a SQL family, test classification, implicit commit,
transaction/residue/lock effects, and cleanup-only preflight. Run the
[session verification](session.md#verification) for interruption and concurrency
regressions.
