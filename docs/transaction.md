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

### Oracle: the mode is re-applied at every transaction boundary

Oracle expresses both halves of the mode as properties of the TRANSACTION
(`SET TRANSACTION READ ONLY` / `SET TRANSACTION ISOLATION LEVEL ...`), so the
mode dies with the transaction it was applied to. A `COMMIT`/`ROLLBACK` inside
the user's own batch, an auto-commit, or a DDL's implicit commit therefore ends
it mid-batch, and applying the mode once per execution would leave every
statement after that point running under the session default while the toolbar
still claims the pinned mode. Both Oracle batch loops re-apply the mode at the
start of the next transaction inside the same batch (the thin loop keys off its
retained state, the OCI loop off the cleanup guard's known-clean flag), and
neither injects it in front of the user's own transaction-first statement,
which has to be first in its transaction itself (ORA-01453).

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
a backend has no statement for (Oracle cannot run a READ COMMITTED read-only
transaction; Serializable + Read only IS expressible — it is exactly what
`SET TRANSACTION READ ONLY` provides). `update_transaction_mode_from_controls()`
checks `DatabaseConnection::transaction_mode_selection_error()` and refuses the
pair where it is chosen, instead of pinning a mode that makes every later
statement fail. The query-driven adoption path applies the same rule: a
session-persistent statement whose merge with the tab's mode lands on an
unexpressible pair is not adopted
(`adopt_session_transaction_mode_change_after_statement`), leaving the
conservative session residue in place.

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

### MySQL/MariaDB: an already-correct session is left alone

The MySQL family acquires the tab's pooled session once per statement, and
preparing it is not free of meaning: the setup statements below start with
`ROLLBACK`, so preparing a session that is already correct ends the transaction
the tab's own reads opened. Two plain `SELECT`s of one script could not share a
snapshot under a pinned isolation level because of it — the level was in force,
but each statement got a transaction of its own. Two rules keep the tab's
transaction intact:

- the reusable-session readiness check applies the connection's session
  settings **without** its default isolation level
  (`apply_mysql_session_settings_without_default_isolation_for_db_type`); the
  execution applies the tab's effective mode straight after, and that mode
  already resolves `Default` to the connection default, so re-asserting it
  first only creates a change that has to be made,
- `apply_mysql_pooled_execution_session_settings` reads the session's
  `autocommit`, isolation level and read-only flag back and skips the setup
  entirely when they already match. Reading system variables touches no table,
  so the probe neither starts a transaction nor disturbs one.

The exception is a statement that must be the first of its transaction (a
one-shot `SET TRANSACTION ...`): the session is prepared back to a boundary for
it even when its settings already match, or the server refuses it with
ER_CANT_CHANGE_TX_CHARACTERISTICS.

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

The commit/rollback/discard PROMPT appears only for actions that end the
session's life: tab close, app exit, disconnect/reconnect, and pool resize —
never in the middle of normal work. `ScopeChange` is always `Allow` (scope is
applied to the retained session in place and destroys nothing). An `Execute`
that comes back `RequireResolution` no longer pops a modal either
(`resolve_required_transaction_decision`): an `InvalidSession` — the one state
execution cannot proceed on, and one with no user work a commit could reach —
is discarded silently and the statement runs on a fresh session; every other
blocked state keeps the preserved session and lets the statement run on it,
surfacing problems as ordinary statement errors the user can resolve with
Commit/Rollback.

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

# The three consumer paths that reach the server on their own worker path:
cargo run --bin verify_grid_save_live -- <thin|oci|mysql|mariadb|all>
cargo run --bin verify_import_live -- <thin|oci|mysql|mariadb|all>
cargo run --bin verify_proc_exec_live -- <thin|oci|mysql|mariadb|all>
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
isolation/access pair is reported where it is selected (S13). The pin also has
to refuse DDL, not only DML, and leave no object behind (S14); a tab that
pinned nothing must run under the CONNECTION default, which is the other branch
of `effective_transaction_mode` and the one a user gets from advanced settings
(S15); and on the MySQL family the pinned isolation must govern the FIRST
transaction behaviourally, not just report itself in `@@transaction_isolation`
— MySQL fixes isolation at transaction start, so "applied one transaction late"
reads correct on the variable and behaves wrong (S16). The remaining scenarios
settle what the pin means over time and against the connection: a
connection-default change leaves a pinned tab pinned and behaving that way
while an unpinned neighbour picks the new default up (S17); the pinned
ISOLATION survives a `COMMIT` inside the user's own batch, read behaviourally
with two reads of one transaction bracketing another session's commit — both
with and without that `COMMIT` (S18a/S18b); a READ WRITE pin writes over a READ
ONLY connection default while an unpinned tab is still refused (S19); and a
locking read keeps its lock until the tab resolves it, which is the sharpest
check that the app leaves the tab's own transaction alone between statements
(S20). Three more settle what the pin means around interruptions and session
lifecycle: a cancelled statement leaves the pin in place and it still governs
the session the tab uses next (S21), the pin survives a disconnect and
reconnect and is applied to the new connection's session (S22), and an open
lazy fetch — which holds the tab's session — closes the transaction-mode
controls until it is fetched out or cancelled, through the same
`SqlEditorWidget::transaction_mode_change_blocked_now()` the toolbar asks
(S23). Finally, two settle the controls' own surface: every isolation level
the toolbar offers for a backend really lands on the session, read back from
`@@transaction_isolation` on the MySQL family and behaviourally everywhere —
a dirty read for READ UNCOMMITTED, another session's commit seen inside the
transaction for READ COMMITTED, and one snapshot held for REPEATABLE READ and
(Oracle) SERIALIZABLE (S28); and a READ ONLY pin refuses a locking read
(`SELECT ... FOR UPDATE`), which the same statement running once the pin is
gone proves is the pin's doing (S29). S29 is also where the Oracle client gate
does NOT act: a locking read reads like a query, so it is the server's
ORA-01456 that refuses it — the backstop the gate's own comment promises.
S30 closes the explicit per-transaction escapes: on the MySQL family the
server honours a one-shot `SET TRANSACTION READ WRITE` and
`START TRANSACTION READ WRITE` over a READ ONLY session characteristic, so a
pinned tab refuses those two statements client-side
(`mysql_statement_escapes_read_only_transaction_for_db_type`; the
session-scoped forms stay allowed — they adopt and re-pin the tab honestly).
On Oracle the same scenario exposed that a batch OPENING with a
transaction-first statement replaced the whole batch's mode with the default
to avoid ORA-01453 — which also disarmed the Read only gate and the
re-application for every statement of that batch. Both Oracle loops now yield
only the INJECTION to the user's transaction-first opener; the mode itself
stays the tab's, so the gate still refuses writes inside the user's
transaction and the pin re-applies once it ends.

InnoDB's SERIALIZABLE turns plain reads into locking reads, so the snapshot
pair cannot be used for it on the MySQL family (it would block the other
session instead of reporting a snapshot); the session read-back is the honest
check there. Both new scenarios run in the harness's manual-commit section: an
auto-commit tab ends its transaction after every statement, so no scenario
that reads a snapshot across two executions can live after the harness
restores the connection's auto-commit.

`verify_auto_commit_live` covers the tab-scoped auto-commit model on the same
four backends: the connection default really commits, the menu write path pins
only the active tab, a second tab on the same connection stays manual, the
dirty guard refuses a change and leaves it without effect, a Read only tab
still refuses the write with auto-commit on, and (Oracle) a read-only
transaction is not ended by a piggybacked wire commit. It also covers the
opposite pin — a tab pinned OFF over a connection default of ON keeps its work
rollback-able while its neighbour tab still commits (S9) — the gate the menu
item itself obeys (`TransactionOptionChange` preflight, not the script path of
S3) on a live dirty session (S10), and a script that fails in the middle under
auto-commit: the work before the failure stays committed and nothing is left
for the close prompt (S11). A connection-default change leaves a pinned tab
pinned in both directions while an unpinned neighbour follows the new default
(S12), a `SET AUTOCOMMIT` inside a script governs the statements after it in
the SAME script in both directions without touching the connection default
(S13), and on the MySQL family an explicit `START TRANSACTION` survives
auto-commit ON — its DML is still rollback-able (S14). The toolbar's Rollback
BUTTON is its own path (an async transaction action on the tab's retained
session, not the typed `ROLLBACK` of S1) and the one a user presses out of
habit after a write: on an auto-commit tab it must not appear to take the
committed work back, and must leave nothing for the close prompt (S20).

Note when reading these: the connection's default isolation is READ COMMITTED
(`ConnectionAdvancedSettings::default_transaction_isolation`), not the MySQL
server's REPEATABLE READ, so "Default" on the toolbar is READ COMMITTED there.
S16 derives its expectation from `default_transaction_isolation()` rather than
assuming the server default.

### The consumer paths carry the tab setting too

Grid-edit save, file import, and the object browser's Execute
Procedure/Function are not plain editor statements: the save runs its own
worker (overriding `DbPoolSessionContext::transaction_mode`), the import runs
as `SqlAction::ExecuteScript`, and Execute Procedure emits `SqlAction::Execute`
onto the active tab. Each therefore has to obey the tab's transaction mode and
auto-commit like any other statement, which their own live probes now pin:

- `verify_grid_save_live` — a tab pinned READ ONLY refuses the save and leaves
  the row untouched, unpinning lets the identical save through, and a save on
  an auto-commit tab survives a later `ROLLBACK`.
- `verify_import_live` — a READ ONLY tab refuses the generated import script
  and the target table stays empty; the same script succeeds once unpinned, and
  an import on an auto-commit tab survives a later `ROLLBACK`.
- `verify_proc_exec_live` — a READ ONLY tab refuses a routine that WRITES and
  nothing is written, while a routine that only READS still runs (the pin must
  not over-block), and the same call writes once unpinned; a call on an
  auto-commit tab survives a later `ROLLBACK` too.

A routine call and a grid save leave conservative session residue, so those
probes discard the session before pinning auto-commit — on such a session the
menu item itself would be closed, which is a different promise
(`verify_auto_commit_live` S10).

### Pinning a tab is only half of a mode change

`set_tab_transaction_mode()` records the tab's choice; it does not travel to a
physical session the tab is already holding. `update_transaction_mode_from_controls()`
is therefore a three-step path, and all three steps matter:

1. `validate_transaction_option_change()` — refuse the change outright when the
   retained session cannot take it (this is what keeps the screen honest),
2. `set_tab_transaction_mode()` — pin the tab,
3. `RetainedSessionOptionChangePlan::apply_transaction_mode()` — push the mode
   onto the tab's retained session.

Skipping step 3 leaves the toolbar reading READ ONLY while the session is still
READ WRITE, and skipping step 1 lets that happen whenever the push is blocked.
Note that `transaction_mode_display_mismatch_error` does NOT catch this: it
compares the displayed mode against the resolved tab mode, not against the
physical session. Live probes that drive the controls must use all three steps
(see `set_transaction_mode_like_the_toolbar` in the consumer-path harnesses),
or they pass only when the tab happens to acquire a fresh pooled session.

A routine call, a grid save, and other conservative statements leave session
residue, so step 1 legitimately refuses a mode change right after them until
the user commits, rolls back, or discards.

When adding a SQL family, test classification, implicit commit,
transaction/residue/lock effects, and cleanup-only preflight. Run the
[session verification](session.md#verification) for interruption and concurrency
regressions.
