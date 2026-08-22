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

A scope the server no longer has is TOLERATED on every backend and no longer
tolerated in silence. The current schema/database is only a name-resolution
namespace, the physical session stays valid, and failing every statement —
including the one that would fix the situation — would brick the tab, which is
what live scenario TM S46 pins on all four backends. But tolerating it left the
one promise a tab makes about a statement broken with nothing on screen: Oracle
resolves unqualified names in the LOGIN schema from that point and the MySQL
family in no database at all, while the tab's selector still shows the scope.
Every apply path therefore answers `SessionScopeAssertion` instead of `Ok(())`
— `#[must_use]`, so a caller has to decide — and the batch says it once through
`SessionScopeReport` and the shared catalog message
(`result_messages::session_scope_unavailable`, given the family's own noun by
`DbBackend::scope_unavailable_message`). Latched on the scope NAME, because a
script can move the session and then lose the new scope too. Paths with no
messages pane of their own — Explain, Go to Declaration — refuse instead
(`require_applied`), rather than answering confidently about an object in
another schema; the callers that run no tab's statements say so with
`ignored_without_a_tab`.

The object browser's scope selection is tab-local: a pick lands on the ACTIVE
tab only (each query editor tab owns its own browser card — tree, filter, and
scope — plus one preview card per connection for the dropdown), never on the
connection's own current schema/database or on sibling tabs. A scope change is
applied to the tab's retained session in place (MySQL `USE`, Oracle
`ALTER SESSION SET CURRENT_SCHEMA`), so it is never gated on the session's
transaction state — an open transaction simply continues in the new scope, and
the commit/rollback/discard decision stays where it belongs, at tab close.

**A tab keeps its scope when its connection comes back up.** Scope is a per-tab
setting like auto-commit and the transaction-mode pin, and those two survive a
reconnect (TM S22 pins the mode half live); this was the one of the three that a
successful connect silently reset, so a tab that had been working in `HR` came
back running in the login schema with an empty selector and nothing said about
it. A scope the new server does not have is NOT a reason to drop it —
`SessionScopeAssertion::ScopeUnavailable` already answers that case, tolerating
the statements and saying so once per run (live TM S46), and session preparation
tolerates it too on every backend (Oracle falls back to the login schema on
ORA-01435; the MySQL family resets the session to no database). The one thing
that clears it is a connection that came back as a different DATABASE TYPE,
where a schema name cannot mean what it meant — the sanitization
`effective_transaction_mode` already makes for the sibling pin.

The binding is the source of truth and the card follows it: nothing is pushed
onto the cards at connect, because `start_connection_metadata_refresh` reads the
tab's binding and states its scope on the card as it refreshes. A background
tab's card therefore stays empty until it is activated, which is what a
disconnect leaves behind anyway — its catalog is gone until something reloads
it. The script-`CONNECT` reset (`QueryProgress::ConnectionChanged`) looks like
the same event and is not: there a DIFFERENT connection replaced the tab's, and
the worker has already dropped the batch's scope for the same reason.

The reverse direction — a statement that moves the session (`USE`,
`ALTER SESSION SET CURRENT_SCHEMA`) — goes through one choke point,
`note_batch_scope_change`. It records the new scope where the running batch
keeps it (the cell the statement loop, the end-of-batch re-apply and a lazy
fetch handover all read, or the thin batch's transition context) and reports it
to the window exactly once, by `QueryProgress::ScopeChangedNotice`, carrying the
scope the statement itself selected. Recording and reporting are one step
because they were two: sites that reported without recording left the rest of
the script running in the scope the tab had when the run started.

Both Oracle drivers also write the new scope onto the tab's BINDING from the
worker, through `record_batch_scope_on_tab_binding`, which asks the two
questions that make a worker's write the tab's to make: is this execution still
the one the tab is on (`SessionHandBackOwner::is_current`), and is the tab still
bound to what this batch resolved (`TabConnectionBinding::set_scope_if_revision`
— the worker's door; the bare `set_scope` belongs to the UI thread, which owns
the tab). Neither question implies the other: a rebind does not move the
operation id, and a new execution does not move the revision. TWO sites write it
from a worker and no other may: the OCI `ALTER SESSION SET CURRENT_SCHEMA` and
its thin twin — both drivers hold ONE session for the whole batch, so where that
session sits is a fact about the tab for as long as the batch runs.

The MySQL family's `USE` is deliberately not among them: it records only its own
batch cell and leaves the tab's binding to the window. That family re-acquires
its session per statement, so the batch cell is what the rest of the script
asserts, the retained lease records where the session really ended up, and
`ScopeChangedNotice` moves the tab and its card. (This said THREE and named "the
MySQL family's `USE` command" first. It was counting a `USE` implementation
inside the ORACLE batch loop, whose comment read "MySQL USE command" and which
could never run — a batch there holds an Oracle connection. That decoy also had
a guard requiring it to write the connection's stored database and bump the pool
epoch, which is what round 9 removed from the live MySQL loop for splitting
sibling tabs off their database. There is now one `USE` implementation, and the
Oracle loop's arm only says the command belongs to another family.)

A refused write says which of the two questions refused
it, because a tab naming one schema while a live session sits in another is the
state this whole rule exists to keep explicable. The batch's own record of the
scope is never gated on either question — the session really did move, so the
rest of that batch must assert the new scope whatever the tab has since done.

The window applies the report unless a LATER execution already owns the tab: an
abandoned operation's notice is still a FACT about the tab's session and must
land (a terminate racing the worker otherwise left the tab naming a schema its
session had left, which the next statement's assertion made true again), while a
superseded one describes a session the tab no longer has
(`query_operation_was_superseded`). That report moves the originating tab's
BINDING and browser card, and nothing else — it
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
statement it cannot classify. `SelectLike` and `TransactionControl` pass, and so
does `SessionControl` once the statement's REACH has been asked about;
everything else does not. `ToolCommand::RunScript` (`@file`) is refused because
its contents cannot be checked first, and `ToolCommand::Connect` because it would
leave the connection behind.

A `SqlKind` cannot answer the reach question by itself, because it is about the
TRANSACTION: Oracle's `ALTER SYSTEM` is session control (it carries no implicit
commit) and `SET GLOBAL TRANSACTION ...` is transaction control, and neither
answer says whether the effect leaves the session. Letting the kind decide alone
passed both of those on a connection the user had marked read-only, while the
tab's own READ ONLY pin refused the first — one app, two answers to "does
read-only allow this?". Two questions are therefore asked ahead of the kind, and
both guards ask the first from one place:

- `sql_classification::statement_reconfigures_the_server` — Oracle `ALTER
  SYSTEM`, and the MySQL family's `SET GLOBAL`/`SET PERSIST`, `FLUSH`, `KILL`,
  `SHUTDOWN`, `RESET MASTER|PERSIST|QUERY`, `PURGE BINARY LOGS`,
  `ALTER INSTANCE`, every `CLONE` form, `CACHE INDEX`, `LOAD INDEX INTO CACHE`,
  `INSTALL`/`UNINSTALL` in every spelling (PLUGIN, COMPONENT, MariaDB's SONAME)
  and replication control. Neither
  server refuses these for a read-only TRANSACTION, so a guard that leans on the
  server lets them through: Oracle's own list of what a read-only transaction
  permits includes `ALTER SYSTEM`, and the MySQL family's read-only session
  characteristic constrains table writes, not server administration. What belongs
  in the list is decided by that one question and nothing else — account and
  privilege statements (`CREATE USER`, `GRANT`, `SET PASSWORD`) and every other
  data-dictionary DDL are deliberately NOT here, because they write tables and a
  read-only transaction refuses them, so both guards get their answer from the
  server.
  `SqlEditorWidget::transaction_mode_refusal_for_statement` asks it first and on
  EVERY backend, which is what closed the same hole in the tab's pin — the
  Oracle allowlist had kept `ALTER SYSTEM` out by omission, and the MySQL
  family, which delegates to the server, refused none of them at all.

  Two things it does NOT decide for itself, because deciding them here is how
  the answer drifted from the questions beside it:

  - the SCOPE of a `SET`. It asks whether ANY assignment targets
    `GLOBAL`/`PERSIST`, not whether all of them do. One `SET` may mix scopes —
    `SET SESSION sql_mode = '', GLOBAL net_read_timeout = 31` is one statement
    both servers accept — and it used to be asked through the predicate that
    answers "is EVERY assignment server-scoped?", which is the right question
    for "does this leave session state behind?" and the wrong one here. The two
    quantifiers now sit at the two questions and share one walk over the
    assignments.
  - which statements are replication control. It asks the classifier's own
    `mysql_replication_words`, which matches the NOUN after an optional MariaDB
    `ALL`. A second list written here missed `START ALL SLAVES` / `STOP ALL
    SLAVES`, which is exactly what that rule's comment warns about: the verbs
    are shared with `START TRANSACTION`, so a spelling one list misses is read
    as a transaction of the user's.

  It is asked of ONE statement. Splitting a text into its statements belongs to
  each guard, not to this clause: one unit can hold several (a custom MySQL
  `DELIMITER` makes `SELECT 1; SET GLOBAL …` one statement as far as the
  executor is concerned), and that is true of every question a read-only guard
  asks. This guard splits in `read_only_block_reason`; the tab's pin splits in
  `transaction_mode_refusal_for_statement`. While this clause split for itself
  instead, the clause beside it in the tab's pin — the explicit READ WRITE
  escape — still read the leading words, so the same leading read hid that one.
- a statement that writes a FILE on the server (`SELECT … INTO OUTFILE`,
  `INTO DUMPFILE`) — it reads tables and writes next to the data directory, and
  MySQL 8.0.46 runs it inside a read-only transaction (measured). The classifier
  reads it as the SELECT it starts with, which is why BOTH guards missed it: a
  read is provably a read. `SELECT … INTO @var` stays allowed, because that writes
  session state.
- lock ACQUISITION — taking a lock is not a read, and other sessions wait for
  it. BOTH guards ask this one, from the same place as the clauses above
  (`read_only_shared_refusal`, which answers with whichever clause refused).
  It used to be the connection guard's alone, and asked of one statement KIND
  (`SessionControl`), which left two holes: a tab pinned READ ONLY could hold
  `LOCK TABLES … READ`, a MySQL backup lock, the global read lock, a named
  `GET_LOCK` or an Oracle `LOCK TABLE` — all measured to RUN inside a read-only
  transaction, unlike `LOCK TABLES … WRITE`, which MySQL 8.0.46 refuses — and
  `SELECT GET_LOCK('x', -1)` slipped past the connection's guard as well, because
  a locking function call classifies as a read. The MySQL family's forms are asked of the statement EFFECTS
  (`StatementSessionEffects::acquires_a_lock_other_sessions_wait_for`), where a
  lock that outlives the transaction already has to be tracked and where MariaDB's
  `BACKUP STAGE`/`BACKUP LOCK` were taught alongside `LOCK INSTANCE FOR BACKUP`;
  Oracle's `LOCK TABLE` is named in the classifier instead, because it dies with
  its transaction and recording it as session lock state would leave every tab
  that ran one asking for a resolution it can never clear. The RELEASE forms stay
  allowed for the reason `COMMIT` does — a connection can be marked read-only, or
  a tab pinned, while it already holds one, and refusing the release would strand
  it.

The check runs in `execute_sql_with_mysql_delimiter_after_lazy_cancel`, the one
place every editor entry point funnels through, ahead of both the transaction
preflight and the bind prompt — a refused statement must never ask for
placeholder values first. Everything that reaches the database through the
editor is therefore covered: `Ctrl+Enter`, F5 scripts, selection execution, F6
explain, object-browser Drop/Truncate, CSV import, and the grid's staged save.

For a restriction the server enforces, use the connection's `READ ONLY`
transaction access mode or an account without write privileges.

## A cancel this app sent is not the session's answer

A break / `KILL QUERY` interrupts the call that is **running**. When the work
finished first there is nothing to interrupt, so the cancel is still travelling
when the session stops being that work's — and the next call the session makes
is answered by it. `SessionCancelClaim::deliver` cannot close that window: it
asks whether the session is still this cancel's to act on, which stays true
right up to the hand-back; the statement being over is a different fact.

All three drivers can leave one behind, and the app recorded otherwise until a
live probe said so. Oracle OCI has no reset ODPI-C exposes; a `KILL QUERY` is
the server's; and Oracle Thin — which clears a cancel that was *queued and never
sent* (`reset_pending_cancel`) and drains its break/reset handshake *inside the
call the break interrupted* — writes one in-band `INTERRUPT` marker onto the
socket when OOB is unavailable, and a marker sent with no reader sits there for
the server to answer the next request with `ORA-01013`. So
`SessionCancelResidue` names one answer for all three, and what differs is only
what each ROAD knows: whether it sent a cancel at all
(`after_a_cancel_this_app_sent`), or whether its own call is the cancel's target
(`unless_a_cancel_is_aimed_at_this_call`, for a toolbar COMMIT/ROLLBACK the app
breaks on the spot).

The rule is `session_policy::answer_not_taken_from_our_own_cancel_when`: ask,
and when the first answer is recognisably our own cancel and something of ours
may still be landing, ask **once** more. Once, not in a loop — a second cancel
answer is the session refusing to work — and only about the cancel, so a real
failure is never asked twice.

**The residue is asked when the ANSWER came, never before the call**, and that
is why it reaches the rule as a closure. It has two doors:

- `answer_not_taken_from_our_own_cancel(residue, …)` for a road whose residue
  cannot change while its call runs — a take, a batch cleanup, a lazy-fetch
  close, a per-tab push. Nothing can aim a cancel at the app's own bookkeeping
  call, so the answer is a value.
- `answer_a_call_a_cancel_could_be_aimed_at(driver, is_aimed_here, …)` for a
  call a cancel can be aimed AT — the toolbar COMMIT/ROLLBACK, which the app
  breaks on the spot when the user cancels it. `is_aimed_here` is asked after
  the call and only then, because that is the only moment its answer is known.
  Folded in as a bool read beforehand, it said "no cancel here" about the very
  cancel the user was pressing: the rule re-asked past it and **ran the COMMIT
  they had cancelled**. There is deliberately no value form of that question —
  computing it early is the defect — and every other road in this app reads the
  cancel flag after its call for the same reason.

For a session out of the POOL the app has recognised this since
`DbConnectionPool::acquire_session_untracked`, which throws that session away and
takes another. **A session taken back out of a TAB's slot never comes through
that door, and it is the only kind that carries the user's transaction**, so
every road that speaks to one asks the rule instead:

- the batch cleanup that files the session (Oracle OCI: its health check and its
  transaction probe),
- the take that gets it back — Oracle OCI's ping and its setup statements, the
  MySQL family's readiness ping and session settings, and Oracle Thin's own
  first contact, which it had none of,
- the two lazy-fetch cleanups (`SqlEditorWidget::session_health_after_a_break`,
  which supplies the residue from `LazyFetchBreakRecovery` rather than from the
  driver, because a fetch knows whether a break was sent at all),
- all three per-tab pushes (auto-commit, transaction mode, scope) and the
  toolbar COMMIT/ROLLBACK on every backend, which run a statement on a session
  the tab has just stopped using and whose answer to an error is to DISCARD it.

Thin's first contact is a **ping and never SQL**. On Oracle a transaction begins
with the first executable SQL statement, and the tab's own `SET TRANSACTION` has
to be the first of its own (`ORA-01453`) — a health check there silently
disarmed a pinned tab, which live `verify_transaction_mode_live` S4 catches. A
ping is a TTC call: it consumes the marker and starts nothing.

Every first call on a session taken back out of a tab's slot runs under the
**tab's own query timeout**, on both Oracle drivers. A retained session comes
back carrying whatever call timeout its last batch left on it and the batch
applies the tab's only later, so an unbounded ping or setup statement on a
half-dead socket — one that never resets — held the worker with nothing
published yet for a cancel to reach.

Live: `verify_commit_close_live`, the two stray-cancel scenarios. They are
driven and not raced, for the reason
`SharedDbSessionLease::leave_a_cancel_on_the_retained_session_for_probe` states —
the window cannot be reached by waiting, so the harness SAYS it. They
discriminate on Oracle Thin, whose marker persists on an idle socket; OCI and
the MySQL family absorb a break sent to a fully idle session, and their half of
the same hazard is the intra-frame race `verify_commit_close_live`'s
fetch-on-the-wire scenario covers.

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
- Between the two there is a WINDOW, and it has an owner. A session that has
  left the tab's slot but has not yet reached the code that will run on it —
  a batch resolving its cancel handle and call timeout, a lazy fetch registering
  its handle and spawning a worker thread — belongs to `WorkerSessionOwner`, on
  every backend. Dropping that owner with the session still in it hands the
  session back through the door above, under the state and the scope the window
  began with; an exit that knows better calls `take()` and owns it from there.
  It is a value and not a rule at each exit because the exits could not keep the
  rule: `thread::Builder::spawn` failing dropped the session into the pool,
  where `reset_before_reuse` rolls the tab's transaction back in silence and the
  user is told only that a worker could not start, and a panic did the same. One
  hand-written exit also picked the scope to file the session under, and picked
  the one belonging to the connection a script `CONNECT` had already replaced.
  Nothing at an exit names the state or the scope now, so neither can be named
  wrongly.
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
- A session CLOSED outside those doors owes the same two answers they give, and
  they are one step: `BatchSessionHandBack::release_without_door(carried,
  log_context)` ends what this execution published over the session AND says
  what closing it destroys, and `discard_lazy_fetch_session` is its twin for the
  road whose currency is `lazy_fetch_can_keep_session` rather than the tab's
  operation id. They replaced a reach-only release, and the half that was
  missing is the half that mattered: reporting the loss was left to each call
  site, three of them did it and the rest did not, so the SAME event — the tab's
  work-carrying session being closed — was announced or swallowed depending on
  which step of the acquisition happened to notice. A caller that can release
  without stating what it releases is the shape a transaction disappears
  through. The lazy fetch is the sharpest case: it takes the TAB's session over,
  so the transaction the tab opened before it is on that session, and only the
  SUCCESS road said anything when the fetch was closed — the cancelled and
  failed ones, the ones a user actually reaches, took it in silence.
- What a close destroys has to be READABLE at the close, which is why
  `RetainedSessionOutcome::DiscardPhysical` carries the state (its DB-layer
  sibling `RetainedSessionDisposition::DiscardPhysical` already did). While it
  carried nothing, every road through the MySQL family's disposition — a
  session-info sync that failed, a statement error the session cannot be reused
  after, an interrupted batch whose statement requires a physical discard — had
  nothing to report even if it had remembered to. The Oracle twin is the
  post-interrupt REPLACE (`OracleCleanupSessionDecisionApplier::
  discard_physical_session`), which `decide_session_after_interrupt` answers
  before it looks at the retained state at all — for an unfinished fetch worker
  and for a connection error — so a tab holding an INSERT whose SELECT was
  cancelled had its session closed and was told nothing.
- And the report has to REACH the user. It travels as
  `QueryProgress::RetainedSessionLostWithWork`, not as an error `Message`,
  because the window drops every message of an ABANDONED operation — which is
  right for progress and is exactly the state a worker is in when it reaches a
  hand-back door, since a force-cancelled batch is abandoned rather than joined.
  Sent as a message, the door's own promise reached nothing but the log. The
  filter now asks `tab_fact_delivery`, which separates the OPERATION's progress
  from a fact about the TAB that outlives it: a scope notice and the two per-tab
  pin notices are delivered unless a later execution SUPERSEDED them, and a lost
  work-carrying session is delivered always — no later execution can answer what
  the older one's session took with it. The worker half of the same rule is
  `TabOperationOwnership::may_state_a_tab_fact`, which reads the same two
  counters `query_operation_was_superseded` does, so the writer and the
  deliverer of a tab fact cannot disagree about whether it is stale.
- What a close destroys is what the session is CARRYING, folded from the
  statement that just ran — never the state from before it. All three drivers
  answer it the same way now: Oracle OCI from its batch delta
  (`retained_state_a_discard_destroys`), Oracle thin from its post-statement
  `retained_state`, and the MySQL family from
  `mysql_state_a_close_would_destroy`, which folds the statement's effects with
  the app's own belief instead of a probe (there is none to ask on a session
  being closed). The MySQL answer used to be the PRIOR state, lowered when a
  commit had resolved it and never raised, so an `INSERT` under `autocommit=0`
  from a clean session — the commonest way a tab acquires uncommitted work —
  left the close reporting nothing at all.

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
