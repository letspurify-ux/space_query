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

The object browser's scope selection is tab-local: it moves each bound query
tab's scope, not the connection's own current schema/database. Operations that
run on the shared live connection instead of a pool session — Quick Describe
and Explain Plan — therefore resolve names in the requesting tab's scope
(`DatabaseConnection::oracle_schema_for_scope()` /
`mysql_database_for_scope()`, the same "tab scope, else connection" rule as
`DbPoolSessionContext::for_scope()`). Explain applies that scope to the live
session, so the next operation must reapply its own; Quick Describe never
switches the session — it names the schema/database in the lookup itself,
because MySQL 8 and MariaDB fold `DATABASE()` into a prepared
`INFORMATION_SCHEMA` statement at prepare time and a cached one would keep
answering for the database the session was in then.

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

`assert_connection_lifecycle_closes_every_server_session` proves this against a
live database by counting the server's own sessions (`information_schema.processlist`
/ `v$session`) around each event: discard, return-and-reuse, tab close, an
orphaned lease, disconnect, reconnect, pool resize, a connection dropped without
being disconnected, a connection attempt that is thrown away, and three
connect/disconnect cycles. Each backend joins by
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
because a leak of one session per tab is invisible in a single pass. A script CONNECT's own connection is torn down by dropping the
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
