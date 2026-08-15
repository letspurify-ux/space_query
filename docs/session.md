# Database Session Lifecycle

> Implementation: `src/db/session_policy.rs`,
> `src/ui/sql_editor/execution.rs`, `src/ui/sql_editor/mod.rs`,
> `src/ui/main_window.rs`

This document covers execution, cancellation, timeouts, lazy fetch, and the
resulting physical-session decision. The transaction, residue, and lock model
belongs to [Retained sessions](transaction.md).

## Core rule

The logical connection selected by a query tab is separate from the physical
driver session. Cancellation or timeout may discard the physical session while
leaving the logical connection intact. A later execution acquires another pool
session for the same connection profile and generation.

A physical session is reused only after safety is established. Connectivity
alone does not prove that its transaction or session state is clean.

## Operation identity and stale events

At cancellation time, `SqlEditorWidget::cancel_target_snapshot()` creates a
`CancelTargetSnapshot` containing:

- Tab and editor IDs
- Operation ID
- Connection generation
- Database type and `SqlKind`
- `ExecutionState`, `LazyFetchState`, and auto-commit

Completion uses `ExecutionFinishedEvent` with the same identity. If the
operation ID or connection generation no longer matches, the policy returns
`SessionDecision::IgnoreStaleEvent`. An old worker must never overwrite a newer
operation or connection.

## States and requests

`ExecutionState` distinguishes idle, statement/script execution,
lazy-fetch-only, cancellation/closing, and unknown states. `LazyFetchState`
models this lifecycle:

```text
None -> Waiting <-> Fetching -> CloseRequested/CancelRequested -> Closed
```

UI-level `LazyFetchRequest` and worker-level `LazyFetchCommand` are separate.

| UI request | Worker behavior |
| --- | --- |
| `More` | `FetchMore(batch_size)` |
| `All` | `FetchAll` |
| `Cancel` | Graceful close or fetch cancellation, preserving reuse eligibility |
| `CancelAndDiscard` | Cancellation with physical-session discard intent |

Worker commands separately represent `GracefulClose`, `CancelFetch`, and
`ForceCancel`. A waiting cursor must be closed before reuse. A fetching cursor
requires both fetch-worker termination and cursor closure.

## Interruption classification

The UI execution path distinguishes these `InterruptKind` values:

- `Cancelled`
- `RecoverableTimeout`
- `NonRecoverableTimeout`
- `ConnectionError`
- `UnsafeOrUnknown`

`is_recoverable_timeout()` requires select-like SQL, a known lazy-fetch state,
and a database-specific driver marker. A generic timeout message synthesized by
the application is not enough. MySQL/MariaDB lock-wait timeout is a separate
transaction case and is not treated as a normal recoverable query timeout.

## Post-interruption decisions

`decide_session_after_interrupt()` maps `InterruptDecisionContext` to a
`SessionDecision`.

| Decision | Meaning |
| --- | --- |
| `IgnoreStaleEvent` | Ignore completion unrelated to the current operation |
| `ReuseSamePhysicalSession` | Keep the same session after cleanup and health check |
| `ReplacePhysicalSessionKeepUiConnected` | Discard physical session, keep logical connection |
| `RequireCommitOrRollback` | Retain session for transaction resolution |
| `RequirePhysicalSessionResolution` | Require residue/lock cleanup or discard |
| `MarkDirtyAndBlockNextExecution` | Block on an uncertain transaction-control result |

Normal reuse requires every applicable condition below:

- Snapshot operation and generation still match.
- The worker has ended and no connection-fatal error occurred.
- SQL classification and statement effects permit reuse.
- Lazy cursor and fetch worker cleanup are confirmed for the current state.
- A timeout is recoverable and temporary timeout settings were restored.
- Both the ping and `SELECT 1` in `health_check_session()` succeed.

DDL, session control, unsafe DML/PLSQL/script, and unknown SQL do not use this
normal reuse path. Existing retained state takes precedence over replacement.

## Database-specific cancellation

- OCI Oracle calls `break_execution()` on `Arc<Connection>` and closes with
  `CloseMode::Drop` on forced termination.
- Thin Oracle calls `OracleThinCancelHandle::break_execution()` and uses
  `force_close()` on forced termination.
- MySQL/MariaDB uses `KILL QUERY` through a separate control connection/context,
  followed by `KILL CONNECTION` for forced termination.
- A pool acquisition may be cancelled before a physical session exists. The
  operation snapshot still rejects late events in that case.

Driver-specific cleanup is reduced to a shared `SessionDecision`, and
`apply_session_decision()` updates the editor's retained and replace-pending
state.

## Next execution and scope

A reusable retained lease must match connection generation, pool-context epoch,
database type, and current schema/database. Oracle current schema and
MySQL/MariaDB current database are reapplied to new sessions. A running worker
is not mutated during a scope change; the scope takes effect at the next safe
acquisition or reuse point.

The database a SESSION sits in and the database the CONNECTION hands to tabs
that asked for none are two different values, and a statement that ends one does
not end the other. A tab with a scope of its own can `DROP DATABASE` the one it
is sitting in while the connection's own database is untouched, so the answer to
"whose database just went away?" is decided where the statement is read
(`mysql_session_database_update_after_statement`) and travels to the sync as a
value. Recording the session's move on the connection cleared the default every
scope-less tab followed, and those tabs then answered "No database selected" for
a database nobody dropped.

Every batch puts its session in the requesting tab's scope before it runs a
statement — Oracle OCI through `apply_oracle_schema_before_pooled_action`,
Oracle Thin through `apply_oracle_thin_schema_before_statement`, the MySQL
family through `apply_mysql_global_database_before_pooled_action`, each
resolving the target with its family's one rule. That assertion, not the eager
push, is what makes the tab's scope the truth: a pooled session arrives
carrying whatever its last user left on it, a session retained from the tab's
previous run carries whatever THAT run left, and a statement can move it where
the app cannot see it (`EXECUTE IMMEDIATE 'ALTER SESSION SET CURRENT_SCHEMA
...'` is not the spelling the adopt path matches). Applying a scope change to
the retained session immediately is therefore a convenience, not a correctness
requirement — it needs the connection lock and gives up silently when it
cannot take it. Thin used to assert nothing at all, applying a schema only when
it acquired a fresh pool session, so the same script answered differently on
the two Oracle drivers and a dropped scope change left that tab executing in
the old schema for good. The thin target is resolved by
`oracle_thin_batch_session_schema` when the run starts and again when a script
`CONNECT` replaces the connection, rather than per statement, because the thin
batch deliberately runs without the connection lock its OCI twin takes each
time. A single-statement thin SELECT never enters that loop — it streams from
its own lazy-fetch worker — so the schema is resolved for it at the same point
and applied inside the worker (`start_oracle_thin_lazy_select`) right after the
transaction mode; without that it ran wherever the tab's retained session had
been left. On the MySQL family the assertion decides through
`mysql_pooled_session_scope_application`: it must not repeat `COM_INIT_DB` on a
session already in the target (that clears the diagnostics area), but a session
that is somewhere else is moved even when it carries work — skipping every
preserved session instead made the eager push a correctness requirement it
cannot meet. Guard: `every_batch_holds_its_session_in_the_requesting_tabs_scope`;
live check S44 in `verify_transaction_mode_live` (all four backends).

The object browser's scope selection is tab-local: a pick lands on the ACTIVE
tab only (each query editor tab owns its own browser card — tree, filter, and
scope — plus one preview card per connection for the dropdown), never on the
connection's own current schema/database or on sibling tabs. A scope change is
applied to the tab's retained session in place (MySQL `USE`, Oracle
`ALTER SESSION SET CURRENT_SCHEMA`), so it is never gated on the session's
transaction state — an open transaction simply continues in the new scope, and
the commit/rollback/discard decision stays where it belongs, at tab close.

The reverse direction — a statement that moves the session (`USE`,
`ALTER SESSION SET CURRENT_SCHEMA`) — goes through one choke point,
`note_batch_scope_change`. It records the new scope where the running batch
keeps it (the cell the statement loop, the end-of-batch re-apply and a lazy
fetch handover all read, or the thin batch's transition context) and reports it
to the window exactly once, by `QueryProgress::ScopeChangedNotice`, carrying the
scope the statement itself selected. Recording and reporting are one step
because they were two: sites that reported without recording left the rest of
the script running in the scope the tab had when the run started. That report
moves the originating tab's BINDING and browser card, and nothing else — it
must not re-apply the scope to the session, which the statement that emitted it
has already moved. Doing so took the tab's retained lease out of its slot from
the UI thread while the batch that owns it was still running, and the MySQL
family re-acquires that lease per statement: the next statement could find no
session, run on a fresh one, and split the user's open transaction across two
physical sessions. The two sibling per-tab options, auto-commit and transaction
mode, already refuse to touch a session while its tab is executing
(`transaction_option_block_message`). A tab's `USE` deliberately does NOT
write the connection's stored database (that name is the connection's own, and
is what a tab with no scope of its own falls back to), so any second event built
from it would name another tab's database and overwrite the correct one.

A card is loaded from the database only when there is nothing to inherit. The
metadata belongs to the connection and the scope it was read in, not to the tab
that asked first, so a new card copies a sibling card's `ObjectCache`, scope
list and selection outright (`adopt_metadata_from`), and a load that lands
fills every still-empty card of the same connection and scope
(`fill_empty_sibling_cards`). A plain tab switch therefore reloads nothing and
restores the tab as it was — tree, expansion, filter, scope selection — and the
tab's editor takes its completion and highlighting metadata from its own card
(`seed_active_tab_editor_metadata_from_browser`) instead of waiting for a
refresh. Only a first card on a connection or a scope change triggers a load,
and a schema load is written to the editor tabs that share its scope, never to
a sibling tab sitting on another one. Regression harness:
`cargo run --bin verify_object_browser_tabs_ui` (no database needed).

Operations that
run on the shared live connection instead of a pool session — Quick Describe
and Explain Plan — therefore resolve names in the requesting tab's scope
(`DatabaseConnection::oracle_session_schema_for_scope()` /
`mysql_database_for_scope()`, the same "tab scope, else connection" rule as
`DbPoolSessionContext::for_scope()`). The Oracle rule resolves to a concrete
name rather than to "leave the session alone", so neither operation can inherit
the schema another tab left the shared session in. Explain applies that scope to
the live session, so the next operation must reapply its own; Quick Describe
never switches the session — it names the schema/database in the lookup itself,
because MySQL 8 and MariaDB fold `DATABASE()` into a prepared
`INFORMATION_SCHEMA` statement at prepare time and a cached one would keep
answering for the database the session was in then.

Oracle's Explain also *writes* there: `EXPLAIN PLAN FOR` is an INSERT into
`PLAN_TABLE`. A tab pinned **Read only** therefore refuses F6 exactly as it
refuses a write from the editor — the explain path asks the same
`SqlEditorWidget::transaction_mode_refusal_for_statement` both Oracle batch
loops ask, about the statement the backend will actually send
(`ExplainPlanBackend::explain_statement`), not about the `SELECT` being
explained. Asking with the user's text would answer "this is a read" and let
the write through, which is what it used to do. MySQL/MariaDB `EXPLAIN` is a
read and their mode lives on the session, so the same call answers `None` there
and the pin does not block it. No query tab owns the live session, so nothing in the transaction
model would ever resolve that write — a tab's auto-commit governs its own pooled
session, and Commit/Rollback act on the tab's retained session by design — and
it stayed an open transaction holding its rows and their locks for the life of
the connection, growing with every F6. `QueryExecutor::get_explain_plan` and
`get_thin_explain_plan` therefore roll it back themselves, in the function that
issues the statement, after the plan rows have been read: user work never lives
on this session, so there is nothing else for that rollback to reach. Guard
`oracle_explain_plan_resolves_the_write_it_leaves_on_the_shared_session`; live
check in `verify_explain_plan_live` (both Oracle drivers), which reads
`v$transaction` on that very session after an F6.

## Read-only connections

`ConnectionInfo::read_only` is a guard inside this process, not a server-side
lock. Note that two statement shapes classify as writes despite reading:
`SELECT ... FOR UPDATE` (`classify_select_sql_for_db_type` returns `Dml` for a
locking select) and Oracle `EXPLAIN PLAN FOR` (`classify_explain_sql_for_db_type`
returns `Dml`, because it inserts into `PLAN_TABLE`). Both are therefore refused
on a read-only connection, which also means F6 Explain Plan is unavailable there
on Oracle. `sql_classification::read_only_block_reason` splits the text with the same
splitter and MySQL delimiter the executor will use, classifies each statement on
its own, and refuses anything that is not provably a read — including a
statement it cannot classify. `SelectLike`, `SessionControl`, and
`TransactionControl` pass; everything else does not. `ToolCommand::RunScript`
(`@file`) is refused because its contents cannot be checked first, and
`ToolCommand::Connect` because it would leave the connection behind.

The check runs in `execute_sql_with_mysql_delimiter_after_lazy_cancel`, the one
place every editor entry point funnels through, ahead of both the transaction
preflight and the bind prompt — a refused statement must never ask for
placeholder values first. Everything that reaches the database through the
editor is therefore covered: `Ctrl+Enter`, F5 scripts, selection execution, F6
explain, object-browser Drop/Truncate, CSV import, and the grid's staged save.

For a restriction the server enforces, use the connection's `READ ONLY`
transaction access mode or an account without write privileges.

## Teardown closes every session

A session the app has finished with must be gone from the *server*, not merely
gone from the app's own bookkeeping. Two things make that non-obvious:

- A retained session belongs to a query tab, not to the pool, so the pool
  cannot reclaim it. On the MySQL family it also keeps its pool alive, because
  an outstanding `PooledConn` owns a clone of the pool — one forgotten lease
  therefore holds every idle session in that pool open as well.
- The pool-context cache holds a clone of the pool too, so an entry left behind
  by a disconnect does the same thing on its own.

The guarantee is anchored on the connection generation, which moves exactly
when the physical connection or its pool is replaced or closed (connect,
reconnect, disconnect, pool resize):

- Generations come from a process-wide counter, so a generation identifies one
  incarnation of one connection and no teardown can match another connection's
  sessions.
- `bump_connection_generation()` reclaims what the ended incarnation leaves
  behind, on the connection-cleanup worker: it drops every stale cached pool
  context and discards every session retained under the retired generation,
  through the same `DbSessionLease::discard_physical` choke point every other
  discard uses. No call site can opt out of it.
- `DbSessionLeaseEntry` closes its session on `Drop`, so a lease slot that goes
  away without being cleared — a tab that vanished rather than closed — cannot
  leave its session behind either.
- A pool session that was acquired but could not be handed over
  (`discard_stale_session`) is discarded on every backend rather than returned
  to the pool half-configured.
- A worker hands the session it was holding back through one door,
  `SharedDbSessionLease::hand_back_worker_session`, which names the execution it
  belongs to (`SessionHandBackOwner`). A force-cancelled batch is ABANDONED, not
  joined — the tab publishes idle while the worker is still unwinding — so the
  generation and the pool epoch cannot tell the dead batch from the new one:
  both run on the same connection. A hand-back whose tab has moved on closes the
  session instead of filing it, because filing it costs the NEWER batch its own
  session (the lease's conflict resolution keeps whichever arrived first), and
  it answers whether the session it closed carried uncommitted work so the
  caller can say so rather than lose it in silence — including when the SLOT
  refused a session it was asked to retain because the tab had closed, which
  closes it just as surely.
- TAKING the session is the third discard road, and it needed the same answer.
  An entry that belongs to another incarnation of this connection is CLOSED by
  the take, and answering `None` for that made it indistinguishable from an
  empty slot — so every caller read "there was nothing to do" about a session it
  had just destroyed. The close prompt's **Commit** reported success for a
  commit it never ran and then closed the tab; the scope, auto-commit and
  transaction-mode pushes answered `NoSession`, which does not alert. Rollback
  and Discard hid it, because for them the destruction happens to be the outcome
  the user asked for, so the answer was true by accident.
  `RetainedLeaseTake::{Empty, Taken, Unreachable}` now says which of the three
  happened, `lost_work()` answers the same question `SessionHandBack::lost_work`
  does, and `RetainedSessionCloseOutcome` gives a session-ending action the
  third answer a `Result<(), String>` had no room for. An action that could not
  reach the session REPORTS and carries on: nothing is left to retry, and
  refusing would leave the loss unexplained and the close half done.
- The DISCARD direction has the same door,
  `SharedDbSessionLease::clear_worker_session`. A worker leaving a connection
  (script `CONNECT`/`DISCONNECT`, or a batch that ended disconnected) drops
  whatever session the tab had for it, and an abandoned batch reaching that code
  after the tab reconnected would otherwise close the session the user is
  working on now, with no message at all. The binding has the same rule:
  `detach_if_revision` refuses to unbind a tab that has moved on, and a refused
  detach keeps the STALE revision so the superseded batch's later `CONNECT`
  cannot rebind the tab either. There is no unconditional `detach()` beside it,
  because remembering to hold the revision is not a rule that holds: two of the
  three script CONNECT/DISCONNECT undo paths held it and the third — the thin
  `CONNECT` whose `replace_pooled` failed — did not, and an abandoned batch
  reaching it after the tab reconnected would take the tab off the connection
  the user is working on now. Undoing a bind holds the revision that BIND
  produced, not the one the worker started with.

`assert_connection_lifecycle_closes_every_server_session` proves this against a
live database by counting the server's own sessions (`information_schema.processlist`
/ `v$session`) around each event: discard, return-and-reuse, tab close, an
orphaned lease, disconnect, reconnect, pool resize, a connection dropped without
being disconnected, a connection attempt that is thrown away, three
connect/disconnect cycles, and two live connections at once — disconnecting one
closes exactly its own sessions while the other's retained session stays put,
which is the isolation the process-wide generation exists to provide. Each backend joins by
handing the engine a connection and a census; the probe connects through an
identity of its own (a database of its own on the MySQL family, a user of its
own on Oracle) so the count sees this test's sessions and nobody else's.

`verify_session_leak_live` does the same for the holders that only exist once a
real query tab is running, which the lease-level engine cannot see: the
lazy-fetch worker owns its session in its own frame rather than in the tab's
lease slot, a cancelled statement leaves the session wherever the force close
left it, and a statement still on the server when the connection goes away is
owned by nothing the teardown can reach — only the stale sweep can retire that
work and take its session with it. It also runs five tab open/close rounds,
because a leak of one session per tab is invisible in a single pass; a
cancel-and-discard with the tab left open, because everywhere else a tab close
or disconnect stands behind the discard as a second net; a disconnect issued
the instant after a cancel, so the cancel watchdog and the teardown race; and a
reconnect over a live connection while a lazy fetch is still open. A script CONNECT's own connection is torn down by dropping the
tab's binding, which a harness cannot do (it cannot destroy the FLTK widget), so
that path is covered by the engine's drop-without-disconnect case instead.

## Verification

```sh
cargo test session_policy --lib
cargo test lazy_fetch --lib
cargo test cancel --lib
cargo test read_only --lib
cargo test --test concurrency_multithread_guards
cargo test --test db_dispatch_guards

# Live, one database container at a time: every lifecycle event closes every
# session, and a discarded session hands its pool slot back.
cargo test --lib -- --ignored --exact \
  db::connection::tests::mysql_connection_lifecycle_closes_every_server_session \
  db::connection::tests::mysql_discarded_sessions_release_their_pool_slots
# ... and the mariadb_ / oracle_oci_ / oracle_thin_ forms of the same two names.

# Live, same container: no query-tab lifecycle event leaves a session open.
cargo run --bin verify_session_leak_live <thin|oci|mysql|mariadb>

# Live: a read-only connection refuses writes and the database is unchanged.
cargo run --bin verify_read_only_live all
```

A new cancellation path needs tests for stale operation, stale generation,
waiting/fetching cleanup, connection-fatal errors, timeout restoration failure,
and dirty retained state.
