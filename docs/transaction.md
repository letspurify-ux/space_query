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

### One executor UNIT can hold several statements, and the ledger answers for all of them

Every rule above reads the LEADING words of the statement it is given, which is
only true of a single statement. A unit is not always one: a custom MySQL
`DELIMITER` makes `SELECT 1; INSERT INTO t VALUES (1)` one statement as far as
the executor is concerned, `CLIENT_MULTI_STATEMENTS` is on, and the server runs
both. Read from the leading statement, such a unit answered *"this left nothing
on the session"* — so an open transaction, a temporary table, a prepared
statement and a changed charset all went back to the pool unrecorded, the tab was
never offered the commit, and the next statement's session preparation (whose
first setup statement is `ROLLBACK`) threw the work away. `SqlKind::Script` is the
one kind the connection's read-only guard refuses outright, which is why the same
blindness never reached it.

`StatementSessionPostProcessor` therefore has two methods and a backend
implements only the first: `effects_for_single_statement()` answers about ONE
statement, and `effects_for_sql()` — not a backend's to override — splits the
unit with the executor's own splitter and folds the answers in the order the
statements run. A unit holding one statement answers exactly as it always did:
the fold is the identity over one element, and it answers about the text AS
GIVEN, because the splitter normalizes what it returns (it strips comments, and a
MySQL executable comment such as `SELECT … /*!80000 FOR UPDATE */` carries
statement text the server really runs).

The fold does not guess the NET effect, because the net effect is not readable
from a merge: `INSERT; COMMIT` and `COMMIT; INSERT` leave different sessions and
merge identically. It keeps the model's own rule instead — everything that ADDS
state or uncertainty is taken from either statement, and nothing that says
something ENDED is claimed from either, because a claim is only lowered by an
ANSWER. Consuming a pending one-shot `SET TRANSACTION` is the exception that
proves the rule: consumption is permanent, so it is folded rather than dropped.

Only a unit that really DROPPED an ending is marked as one the app could not
read (`may_open_untracked_transaction`), which is what sends the batch end to the
server for an answer instead of filing a guess. With nothing in the unit ending
anything the fold is exact and nothing is guessed — otherwise every script
written under a custom `DELIMITER` would ask its tab to commit two plain reads.

The same split serves every other question about what a unit left behind —
`fold_over_unit_statements()`, asked by the transaction-mode adoption, the
auto-commit adoption, the `USE` that moves the session's database, and the
`DROP DATABASE` that takes it away. Each of those used to read the leading
statement too. Guard:
`every_question_about_what_a_unit_left_on_the_session_asks_every_statement`.

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
  connection's default. Everything else the mode is expressed with comes from
  the new connection too — including its default isolation, which is what a tab
  that selected `Default` asks the session to be put back to (see below).
  Keeping the value read at execution start made both Oracle drivers express
  the REPLACED server's level on the new one while the toolbar showed the new
  connection's.

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

Both adoptions — this one and auto-commit's — are asked of every statement in
the unit, from the split above, and the LAST statement that sets a value is the
one the session was left with (the two halves of the mode merge separately, so
`SET SESSION TRANSACTION ISOLATION LEVEL SERIALIZABLE; SET SESSION TRANSACTION
READ ONLY` in one unit adopts as both). Read from the leading statement,
`SELECT 1; SET SESSION TRANSACTION ISOLATION LEVEL SERIALIZABLE` moved the
SESSION while the tab's pin, the toolbar and the Tools menu kept the old value —
the screen-equals-session guarantee broken by a leading `SELECT 1;`, with the
override residue never adopted away either; its auto-commit twin left the app
offering Commit and Rollback for work the server had already committed.

A batch that adopts is not always the batch the TAB is on. A force-cancelled one
is ABANDONED rather than joined — the tab is published idle while the worker is
still unwinding, and abandoning it clears the cancel flag, so that worker can go
on running the rest of its script while the user's next execution already owns
the tab. That execution resolved its own auto-commit and transaction mode at
startup, where the screen/session checkpoint runs, so a write from the dead
batch is a change nothing downstream would catch.

**Both pins are slots of a type that can only be written through a door.**
`TabPin<T>` holds the slot privately; a worker's one way in is
`record_for_batch(&TabOperationOwnership, value)`, and the UI thread's writes
are named `set_from_ui` / `clear_from_ui` so calling one from a worker reads
wrong. The pins were bare `Arc<Mutex<Option<_>>>` slots written with
`store_mutex_*` from four places each, and the first attempt to close that put a
named door in front of them and a source-text guard behind it — which counted
ONE spelling of the bare write while the three the code actually used went
straight past. A rule a guard has to spell is a rule the next edit can
misspell; this one is the type.

**The question a pin asks is `TabOperationOwnership::may_state_a_tab_fact`, not
`is_current`.** They are two different questions and the type answers both:

- `is_current` — "is the tab ON this execution right now?" — belongs to whatever
  TAKES OVER the tab's live state: its session slot, its cancel reach, and the
  auto-commit its cancel snapshot reports for the RUNNING operation (a per-tab
  slot despite the name, and the third thing these same four call sites write).
- `may_state_a_tab_fact` — "has a LATER execution owned this tab?" — belongs to
  a fact the worker REPORTS about the tab. The user's own `SET AUTOCOMMIT` or
  `SET SESSION TRANSACTION` really succeeded on this tab's session; nothing but
  a later execution's own answer replaces that, and the tab merely being idle
  cannot, because idle is exactly the state a force-cancelled tab is published
  in. It reads the tab's live AND completed operation counters, mirroring
  `query_operation_was_superseded` in the window — a value built without the
  completed counter (which is what the session hand-backs carry, since they only
  ask `is_current`) answers the strict question rather than guessing the loose
  one.

That is the same rule the tab's SCOPE follows, and the two used to disagree: the
scope door asked `is_current` while the window DELIVERED the matching
`ScopeChangedNotice` for an abandoned batch and wrote the very same binding
itself, so which value the tab ended up with depended on which of the two
writers ran. `QueryProgress::AutoCommitChanged` and
`QueryProgress::TransactionModeChanged` are classified the same way
(`TabFactDelivery::UnlessSuperseded`), so the screen follows exactly the pins
the worker moved. All three per-tab settings now answer one question with one
value.

The pins do NOT ask the binding revision, and that is the one place they differ
from the scope: a script `CONNECT` rebinds the tab and both pins deliberately
survive it, while the same event resets the scope. What a refused write does not
stop is the batch's own mode: the session really moved, so the rest of THAT
batch runs under the new value and only the tab is left alone.

**On the MySQL family the adoption is not a batch-loop step but part of ONE
"the statement succeeded" step**, `record_successful_mysql_batch_statement()`,
which also applies the statement's effects to the batch ledger and answers with
the scope change the statement made. The reason is the reason the read-only gate
moved to the session acquisition: that family runs a statement down one of three
paths (the streaming SELECT, the lazy fetch, the plain executor) and the dispatch
between them reads the LEADING keyword of the unit, so a unit the adoption cares
about is classified as a displayable SELECT and takes a path of its own. While
the four steps were spelled out in the plain executor's branch they were simply
not taken for such a unit. The scope change comes back as a `#[must_use]`
`MySqlBatchScopeChange` rather than being reported inside that step, because
recording it where the batch reads its scope and reporting it to the window are
one step (`note_batch_scope_change`) that each branch performs where its own
output order puts it — and a branch that drops the value does not compile clean.
Both Oracle loops still adopt inline: each of them handles every statement of its
batch in one place, so there is no second path to forget.

### A setting change that changes nothing is not a change

Auto-commit and transaction mode are both refused while the session may hold
uncommitted work, and both therefore have to agree on what a change is: a
command or a selection that names the value already in force changes nothing,
so it is neither guarded nor refused. The toolbar states this for transaction
mode (`update_transaction_mode_from_controls` returns before its guard when the
mode is unchanged) and
`SqlEditorWidget::ensure_script_auto_commit_change_allowed` states it once for
every backend's script `SET AUTOCOMMIT`. It used to be stated inline in the
MySQL-family and Oracle Thin branches and was missing from the OCI one, so a
script that repeats `SET AUTOCOMMIT OFF` after a DML stopped on OCI
(continue-on-error is off by default) and ran everywhere else. The guard is
passed as a closure so a no-op also skips the OCI branch's server round trip
asking whether the session holds uncommitted work.

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
start of the next transaction inside the same batch, and neither injects it in
front of the user's own transaction-first statement, which has to be first in
its transaction itself (ORA-01453). That yield is the same at all three
injection sites — batch start, the boundary re-application, and after a script
`CONNECT` — because a mode stated ahead of the user's own `SET TRANSACTION`
fails THEIRS.

"Has this transaction ended?" is answered from what the app tracked, until that
answer stops being knowledge. A PL/SQL block or a `CALL` may commit internally,
which ends the transaction the mode was attached to with nothing else to notice.
From that point the app does not ask a probe, because **no Oracle probe can
answer**: OCI's `DBMS_TRANSACTION.LOCAL_TRANSACTION_ID` and the thin driver's
status flag both report the transaction id Oracle assigns on the first WRITE, so
neither can see the transaction a pinned tab's own `SET TRANSACTION` opens. It
STATES the mode instead and reads the server's reply, through one shared
predicate (`oracle_transaction_mode_boundary_must_be_restated`): the statement
applies, or ORA-01453 says a transaction is already open and the pin belongs to
the next one. Both replies are knowledge, and the round trip is the one the
probe used to cost.

Deciding that and recording what the current statement leaves behind are ONE
call, `OracleTransactionBoundaryTracker::begin_statement`, because they were two
and the two loops ordered them oppositely: OCI recorded after its decision, thin
before it. Thin therefore read the opacity of the statement it was about to run
instead of the one that had just run, so `INSERT; BEGIN COMMIT; END; SELECT …`
made OCI re-apply the pin while thin ran the rest of the batch at the session
default — one script, two answers, from a predicate both drivers shared. The
decision hands out a `#[must_use]` step that is spent where the statement's fate
is known — `ran` past every refusal (the last of which is the per-statement
scope assertion), `refused` at each one — so a statement that never reached the
server records nothing.

Everything the tracker records is MONOTONE, and for two reasons. A plain
`SELECT` after the block does not make the block's commit visible, so no
readable statement may clear the guess. And a statement is recorded BEFORE it is
sent, so nothing at that point knows whether it reached the server or what it
did there: a write refused at parse time wrote nothing, and lowering "this
transaction may be invisible to a write probe" from it let the batch end file a
pinned tab's open transaction as clean. Claims are lowered only by an ANSWER —
the server stating the mode, a replaced session, or the batch-end probe.

That correction reaches the statements of one batch, and the guess is filed with
the session, so both drivers settle it with the server before they file:
`RetainedSessionState::with_transaction_claim_settled_by_server`. It is
deliberately narrow — it lowers only a claim the app could not READ (the
failed-statement rule above stays intact), only on an answer (an interrupted
batch has none), only from `MaybeDirty`, and only when the probe could have SEEN
the transaction in question (`ServerTransactionAnswer::NoWriteTransaction`
settles nothing about a claim the batch flagged as possibly write-less). A claim
left standing is safe because it no longer governs the next batch: every batch
states the tab's mode and lets the server answer, so a stale "a transaction may
be open" costs one round trip and is corrected rather than obeyed. Session
residue is untouched: it is not the transaction, and it is why the session stays
with its tab. The MySQL family has asked the same closing question since the
failed-implicit-commit round; this is Oracle joining it.

A boundary re-application refused with **ORA-01453 is an answer, not a failure**:
it says a transaction is still running, so the pin belongs to the next one,
which is what "applied from the next transaction" has always meant. Reading it
as an error stopped the batch on OCI (after a parse-failed DDL, whose implicit
commit never happened) while thin ran the same script to the end. The mode
statements' effects are also recorded BEFORE the round trip, not after it: a
cancel that lands between the server running `SET TRANSACTION` and the app
reading the answer would otherwise leave an open read-only transaction that
`DBMS_TRANSACTION.LOCAL_TRANSACTION_ID` cannot see, and every later batch would
fail with ORA-01453.

Those rules are not a call site's to keep. All FOUR Oracle sites — the OCI
batch, the thin batch, the thin lazy SELECT and the OCI post-`CONNECT`
injection — state the mode through
`apply_oracle_transaction_mode_statements_with`, which owns the whole shape: the
session-default reset is the one statement it does NOT record, every other one is
recorded before its round trip, ORA-01453 is `TransactionStillOpen`, and anything
else is `Failed`. A **failure states nothing**: only the two ANSWERS reach
`OracleTransactionBoundaryTracker::note_transaction_mode_stated`, because that
call clears the guess and says a transaction no write probe can see may be open.
The thin batch used to make it whatever came back, so a batch whose mode
application really failed filed its session with the claim settled by an answer
the server never gave, while the OCI twin recorded nothing — one script, two
answers, from the code that was meant to make them one.

The post-`CONNECT` injection was the fourth site and the one that did not go
through the function owning that shape: it stated the mode with
`DatabaseConnection::apply_oracle_transaction_mode`, read ORA-01453 as a hard
failure and tore down a connection it had just authenticated for it, told the
tracker nothing about the transaction its own `SET TRANSACTION` had just opened,
and recorded the effects of a DIFFERENT statement list from the one it ran (no
`tab_selected`, no connection default, so a tab that had actively selected
`Default` isolation was not put back to the NEW connection's level). It now
builds ONE `OracleTransactionModeApplication` for both, keeps the server's
ANSWER rather than a flag — so the claim it later makes to the tracker can be
read against the reply it rests on — and the thin driver's equivalent (which
applies the mode lazily after a `CONNECT`) reaches the same function. TM S8, S50
and S55 drive this path.

**Nothing the session carries delays stating the mode.** A batch states the
tab's pinned mode before its first statement whatever the session it was handed
holds, and the two exceptions are about the batch's own STATEMENTS: a leading
script `CONNECT` (the mode belongs to the new connection) and the user's own
transaction-first statement (which must be first in its transaction itself).
There used to be a third, a gate that skipped the mode over a session that "may
have uncommitted work" — from the era when ORA-01453 failed the batch. Once that
refusal became an answer the gate only did harm: "may have uncommitted work" is
a GUESS after any statement whose body the app cannot read, and it is filed with
the session, so a single `BEGIN … COMMIT; END;` made every LATER batch of a
pinned tab skip the pin as well. The tab ran at the session default for the rest
of its life while the toolbar showed Serializable or Read only, and nothing
would ever ask again. The MySQL family needs no equivalent either way: it states
the mode as SESSION state when it prepares the session, and a preserved session
still carries it.

### Read only is one answer, asked by every path that writes

Oracle expresses read-only as a property of the TRANSACTION
(`SET TRANSACTION READ ONLY`), so a `COMMIT` inside the user's own batch ends it
and every statement after it would run read-write. Both Oracle batch loops
therefore refuse non-queries client-side while the tab's access mode is Read
only; the server's ORA-01456 is only the backstop. MySQL/MariaDB need no such
gate — `SET SESSION TRANSACTION READ ONLY` survives the commit by itself.

That answer lives in one place, `SqlEditorWidget::transaction_mode_refusal_for_statement()`,
because it has to be the same whichever button was pressed. It asks the backend
whether the mode is a property of the TRANSACTION here
(`DatabaseType::transaction_mode_requires_first_statement`) and then the
statement classifier (`oracle_read_only_allows_statement`). F6 Explain Plan is
the path that showed why: `EXPLAIN PLAN FOR` inserts into `PLAN_TABLE`, and it
ran without asking, so a Read only tab could still write through it. Every
caller asks about the statement that will actually be sent, which for Explain is
`ExplainPlanBackend::explain_statement()` rather than the `SELECT` being
explained.

The object browser's menus stop OFFERING what that gate would refuse, and the
answer they ask has two sources with different owners: the connection's
read-only flag and the tab's READ ONLY pin. `CardWriteRefusal` holds them apart
and joins them, because a single combined flag has two writers — the runtime
re-labelling re-states the connection's flag on every card whenever a connection
changes state, and the tab's mode sync states the pin on one card — and the
first erased the second's answer every time it ran. There is deliberately no
setter for the combined value. The sync that carries the pin also re-arms itself
when it cannot read the connection (another tab's query holds the mutex) instead
of dropping the request, because a mode a batch just ADOPTED reaches the toolbar
and the card through it and nothing else.

The gate refuses WRITES, not everything Oracle happens to allow. Oracle's own
list of what a read-only transaction permits is SELECT (without FOR UPDATE),
`LOCK TABLE`, `SET ROLE`, `ALTER SESSION`, `ALTER SYSTEM`, COMMIT, ROLLBACK and
SAVEPOINT, and the client allowlist follows it — refusing `ALTER SESSION SET
NLS_DATE_FORMAT` while the app issues `ALTER SESSION SET CURRENT_SCHEMA` on that
same session, and while allowing the ISOLATION_LEVEL form, was an arbitrary line.
`ALTER SYSTEM` and `LOCK TABLE` are the two exceptions kept out: the pin is the
user saying this tab changes nothing, and reconfiguring the instance — or making
every other session wait — is a change whatever the transaction semantics say.

Those exceptions are not the allowlist's to remember, and they are not Oracle's
alone. `transaction_mode_refusal_for_statement` asks
`sql_classification::read_only_shared_refusal` FIRST and on every backend, so the
answer cannot depend on a keyword being absent from a list. That shared answer has
THREE clauses, and it is the whole half the two read-only guards must answer
identically — because a server's read-only transaction does not reliably refuse
these, and which ones it does refuse is a property of the version rather than of
the promise. Measured on MySQL 8.0.46 under `SET SESSION TRANSACTION READ ONLY`:
`SET GLOBAL`, `FLUSH TABLES WITH READ LOCK`, `CACHE INDEX`, `LOCK TABLES … READ`,
`LOCK INSTANCE FOR BACKUP`, `GET_LOCK` and `SELECT … INTO OUTFILE` all RUN, while
`LOCK TABLES … WRITE` and `ALTER INSTANCE` are refused; Oracle permits
`ALTER SYSTEM` and `LOCK TABLE` inside one by its own documented rule. So the app
answers for itself:

- **a statement that reconfigures the SERVER rather than this session** — `SET
  GLOBAL`/`PERSIST` (any assignment of one, not all of them), `FLUSH`, `KILL`,
  `SHUTDOWN`, replication control in every spelling the classifier knows,
  `RESET MASTER|PERSIST|QUERY`, `PURGE BINARY|MASTER`, `ALTER INSTANCE`, every
  `CLONE` form, `CACHE INDEX`, `LOAD INDEX INTO CACHE`, `INSTALL`/`UNINSTALL`
  (PLUGIN, COMPONENT and MariaDB's SONAME — the VERB is the question), and
  Oracle's `ALTER SYSTEM`. What belongs in that list is ONE question — is the
  statement's target the SERVER or the instance rather than this session and its
  data? Account and privilege statements (`CREATE USER`, `GRANT`,
  `SET PASSWORD`) and every other data-dictionary DDL are deliberately NOT there:
  they write tables, so a read-only transaction refuses them and both guards get
  their answer from the server. The statements listed write no table at all.
- **a statement that writes a FILE on the server** — `SELECT … INTO OUTFILE` and
  `INTO DUMPFILE`, which read tables and write next to the data directory. The
  classifier reads them as the SELECT they start with, so this is the one clause
  BOTH guards used to miss: a read is provably a read. `SELECT … INTO @var` stays
  allowed — that writes session state, which a read-only session may do.
- **a statement that takes a LOCK other sessions wait for** — `LOCK TABLES`,
  `FLUSH TABLES WITH READ LOCK`, `LOCK INSTANCE FOR BACKUP`, MariaDB's
  `BACKUP STAGE`/`BACKUP LOCK`, a named `GET_LOCK`, and Oracle's `LOCK TABLE`.
  Asked of the statement EFFECTS for the MySQL family, where a lock that outlives
  the transaction already has to be tracked; named in the classifier for Oracle,
  whose `LOCK TABLE` dies with its transaction, so recording it as session lock
  state would leave every tab that ran one asking for a resolution it can never
  clear. The RELEASE forms stay allowed for the reason COMMIT is — a tab can be
  pinned while it already holds one.

The connection's own read-only guard asks the same function, so the app's two
read-only guards can no longer answer differently
(see [read-only connections](session.md#read-only-connections)). They differ on
DATA by design and only there: the connection refuses anything not provably a
read, while the tab lets the server refuse the writes. The lock clause is the
sharpest example of why the split matters — while it was asked of one statement
KIND (`SessionControl`), `SELECT GET_LOCK('x', -1)` slipped past the connection's
guard as well, because a locking function call classifies as a read.

Reaching the MySQL family took one more step, and it is why that half was missing
in the first place: **nothing on that family's execution path called the shared
answer at all.** Its batch kept a gate of its own that asked only about the
explicit READ WRITE escape, spelled inline. Both questions now live in
`transaction_mode_refusal_for_statement`.

### Where each family asks it, and why the MySQL family does not ask it in a batch loop

Both Oracle batch loops ask once per statement, before they choose how to run it,
and the Oracle thin LAZY select — the one Oracle path that runs a statement
without a batch loop — asks it in the condition that elects it, so a refused
statement simply falls through to the batch, which reports the one answer.

The MySQL family asks it in `acquire_mysql_pooled_session`, the one function
every statement of that family passes to get the session it runs on, and NOT in
any of its executors. That is not a preference. This family runs a statement down
one of three paths — the streaming SELECT, the lazy fetch, the plain executor —
and the dispatch between them reads the LEADING keyword of the unit. A unit can
hold more than one statement (a custom `DELIMITER` makes `SELECT 1; SET GLOBAL …`
one statement as far as the executor is concerned), so a unit the gate refuses is
classified as a displayable SELECT and takes a path of its own. While the gate
lived in `execute_mysql_sql` it was therefore not asked at all for such a unit,
and a READ ONLY tab really moved `@@GLOBAL.net_read_timeout` on MySQL 8.0 (live
**TM S61**, which fails at that baseline while **S60** — the same statement
standing alone — passes). Asking where the session is handed over makes the
answer independent of how the statement will be executed, and a new execution
path inherits it by construction. The grid-edit save reaches the same acquisition
through `run_mysql_pooled_action_with_timeout`.

### Every clause of that answer is asked of every statement, from ONE split

The entry point splits the text with the executor's own splitter and asks the
clauses of each statement it finds. Splitting inside a clause is what the
server-change clause used to do, and the clause beside it — the explicit READ
WRITE escape — still read the leading words, so the same leading read that hid a
server change also hid `SELECT 1; SET TRANSACTION READ WRITE`, which disarms the
pin for the next transaction. A `;` inside a string, a comment or a PL/SQL block
is not a boundary, and a tool command never reaches the server at all
(`read_only_block_reason` owns the two that would leave the connection). The
split passes no custom delimiter deliberately: the caller hands over what the
executor already treats as one statement, so any delimiter in force has been
consumed and the default `;` is what may still be hiding inside it — splitting
with it can only find more statements to ask about, never fewer.

Oracle has no reachable form of this: its splitter ends a statement at `;` or
`/`, so no Oracle unit holds two statements, and the Oracle loops hand the gate
one statement at a time. The fix is common to all four backends all the same,
because the rule is about the question and not about the family.

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

**A CONNECTION PROFILE is not where such a pair is chosen.** Its
`default_transaction_isolation` and `default_transaction_access_mode` are two
CHANNELS, not one mode: `connect_blocking_with_policy` takes the ACCESS half into
`DatabaseConnection::transaction_mode` and sends the isolation through
`sync_default_transaction_isolation` to the pool's session preparation
(`ALTER SESSION SET ISOLATION_LEVEL` / `SET SESSION TRANSACTION ISOLATION LEVEL`).
On Oracle they never meet at all — `transaction_mode_with_default_substituted`
substitutes a connection default into a `Default` isolation for the MySQL family
only. `validate_oracle` used to pair them anyway and ask the selection rule about
the result, which refused `Read committed` + `Read only` — the isolation field's
own DEFAULT plus the one access mode a user changes to make a connection
read-only — for a configuration the connect road forms without complaint, with a
message that never named the fix. Both sides now ask
`ConnectionAdvancedSettings::connection_transaction_mode()`, so a validator
cannot judge a value the runtime does not build; the isolation keeps its own
check, which is whether the backend has that level at all. Guard:
`the_connection_mode_a_profile_is_validated_for_is_the_one_connect_builds`.

### The screen's picture of the active tab's connection has one writer

Everything the window shows about the ACTIVE TAB's connection — which
connection it is, whether it is live, its name, and the auto-commit default the
indicator resolves the tab's pin against — is learned in one place,
`AppState::refresh_active_connection_view()`, and nowhere else writes those
fields. It answers THREE things, not two, because `try_lock_connection` says
`None` both while another tab's query holds the connection mutex and while a
connect/reconnect/disconnect/pool-resize transition is in flight:

- the active tab is bound to no connection: `AppState::connection` points at a
  never-connected placeholder, so every reader answers "not connected" for such
  a tab instead of describing whichever connection was active before it,
- the connection was read: that is the truth,
- the connection could not be read: the runtime answers instead
  (`ConnectionRuntimeState::liveness_without_connection_lock`), a transition in
  flight lowers nothing, and a value may only be KEPT for the connection it was
  learned from.

Reading the unreadable answer as "not connected" filed a live tab as
disconnected whenever a NEIGHBOUR tab was running a query: the status bar lost
its connection, the transaction-mode combos went grey with no retry armed
(exactly the case the deferred re-arm exists for), and the tab's metadata
refresh was dropped. Keeping an auto-commit default across a switch to another
connection made the status bar and the Tools menu show one connection's default
under another connection's tab. The view is re-learned on every status tick as
well as on every tab switch, so a window in which the connection could not be
read closes by itself.

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

**A claim about the screen belongs only to the tab that IS the screen.** Both
recorders state `AppState::sql_editor` — the ACTIVE tab — because there is one
screen; the slots live per tab only because the checkpoint reads them from the
WORKER, which cannot reach the window. The storage was right and the LIFETIME was
missing: a background tab kept the claim it had when it was last on screen, and
both tab-fact handlers re-sync the ACTIVE tab's controls, so a batch adopting a
mode or an auto-commit on a BACKGROUND tab moved that tab's pin and left its
claim behind. The next execution on it — a follow-up table browse is scheduled
from a timer and needs no user action at all — was then refused against a screen
nobody was looking at, and its result tab marked failed.

`SqlEditorWidget::withdraw_displayed_state` gives the claim up, and
`set_active_editor_tab_with_display_stabilization` is where it is called: the one
writer of the active tab that leaves a tab alive and OFF screen (the other sets
it to 0 once the last tab has closed, and a closed tab's claim goes with its
editor). It is not a weakening — `None` is the value the checkpoint already reads
as *no claim*, and a tab with no screen has nothing to disagree with — and the
same function re-states both for the tab that becomes the screen before it
returns. Guard: `only_the_tab_on_screen_carries_a_claim_about_the_screen`.

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

### A failed statement states nothing it did not do

Both servers commit before EXECUTING a DDL statement, but a statement rejected
at PARSE time commits nothing, so a failure can claim neither that the
transaction ended nor that it is still open. It therefore withdraws only the
BATCH's own dirty claim, which the batch-end server probe can restore, and
records the clear as tentative
(`BatchPriorTransactionEffect::ClearUnlessServerDisagrees`). Two things follow
from keeping the tentativeness in the recorded effect rather than beside it:

- a real `COMMIT` earlier in the same batch stays a fact, so a parse error
  after it cannot resurrect the work it committed (an interrupted batch, which
  never reaches a probe, preserves only what was never confirmed),
- a pending one-shot `SET TRANSACTION` is NOT consumed by a failure, on either
  the batch or the per-statement fold. No probe can ask whether the server
  still holds it armed, and clearing the flag on a guess let the toolbar's own
  mode replace skip its server-side consumption — after which the next
  transaction ran the stale one-shot through the pin that had replaced it.

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

### The three per-tab settings answer one question about the tab's work

A gate that refuses on the tab's work asks `AppState::tab_db_work`, which folds
in the lazy fetches the WINDOW holds beside the editor's own.
`TabDbWork::for_editor` is the derivation for a caller that has an editor and
cannot name a tab; it was what the auto-commit and transaction-mode gates asked
while the scope gate asked the other, so one tab could answer "there is work" to
one setting and "there is none" to the other two.

**A control that offers a change and the callback that performs it are one
question**, and the work is a PARAMETER of that question so they cannot derive
it apart. `SqlEditorWidget::per_tab_option_change_blocked_by(work, db_type,
option)` is the gate; `AppState::per_tab_option_change_blocked(tab_id, …)`
supplies the work the window can see and is what both the transaction-mode
combos and the `Tools > Auto-Commit` item ask, while
`transaction_mode_change_blocked_now` supplies the editor's own work for a
caller that has no window (the live harness, which is what TM S23 drives).

Fixing only the callbacks is what re-opened this: the callbacks moved to
`AppState::tab_db_work` and the combos kept deriving their own, so they stayed
live during the window the window holds a fetch the editor does not — the exact
state the gate's own doc says it exists to prevent. The auto-commit item had no
enablement gate at all and simply alerted; it has one now. Both options ask ONE
session rule (`ensure_retained_session_option_change_allowed`), which
`the_transaction_mode_gate_and_the_option_gate_are_one_rule` holds equal to the
control's older spelling on every backend and every state, so the merge cannot
drift either.

The same family rule applies to the SCREEN: `render_status_bar` settles all
three on every status tick. Auto-commit and transaction mode have had that
healer for some time; scope was left to a tab switch, so a worker that moved the
tab's binding while the matching `ScopeChangedNotice` was dropped as superseded
left the selector naming a schema the tab had left, for as long as the user
stayed on it.

`AppState::sync_active_tab_scope_selection` asks for the WHOLE repair, through
`synchronize_scope_for_tab`. Re-stating the selector alone is not a smaller
version of that: `ObjectBrowser::set_selected_scope` compares the name against
what the held catalog was ASKED for and, when they differ, retires the catalog —
so a healer that stopped there discarded the tab's tree and ordered nothing to
refill it. `synchronize_scope_for_tab` therefore decides its metadata repair
from whatever was BEHIND — the binding or the card — rather than from the
binding alone, and retiring a catalog and ordering its reload are one step.

### A gate has two halves: what it asks, and when it is asked

The screen facts above are simply RE-STATED by the tick, which is affordable
because stating them is a `set_value` and a menu flag. The result grid's EDIT
control is the one consumer of these settings that cannot work that way:
`refresh_result_edit_controls` re-lays out the result toolbar, so calling it per
frame would be a layout and a redraw per frame. It was therefore left to events —
~40 query-lifecycle and tab-switch call sites — and when it learned to ask the
tab's READ ONLY pin beside the connection's read-only flag, none of those events
was a per-tab setting moving. Pinning a tab READ ONLY with a grid open left the
checkbox offered, so the user staged edits and met the refusal at Save, which is
the exact state the control hides itself to prevent; unpinning left it hidden;
and a browser scope pick left it offering a save the stale result origin would
refuse.

So the tick carries the ANSWER instead of the restatement.
`AppState::refresh_result_edit_controls_if_their_answer_moved` watches exactly
the two facts these settings move — `active_tab_write_would_be_refused()` and
`active_result_origin_is_current()` — keyed by tab, and re-asks the control only
when one of them changes. It is called from
`sync_transaction_mode_controls`, before that function's two arms, because that
is the one place the tab's effective access mode is derived: it runs on every
status tick, from every road that moves the mode, and it is already the publisher
of the same fact's other half to the tab's browser card. `can_begin_edit_mode` is
deliberately NOT watched — it parses the result's SQL, and every road that moves
it is one of the ~40 that already call the refresh. Guard:
`a_control_that_offers_a_write_asks_every_half_of_the_refusal`.

**Hiding a control may not strand work the user has already staged.** Making the
pin re-ask this control exposed the other half of the same question: the whole
control group was hidden on `can_edit`, which is *may a write be STARTED here* —
and that took the two ways OUT of an open edit session with it, the Cancel button
and the checkbox whose other position cancels. A tab pinned READ ONLY mid-edit
then left the staged rows in the grid with nothing to discard them and nothing to
save them. Abandoning staged work is not a write, so it is not the refusal's to
hide: Insert/Delete/Save follow `can_edit`, while the checkbox and Cancel follow
`edit_active && origin_is_current`, and the checkbox's MARK says whether a session
is OPEN (a shown-but-unchecked box over a live session would begin a NEW edit on
the next click, where the user meant to leave). `origin_is_current` is part of it
because the exit has to WORK: `clone_result_tabs_for_edit_action` refuses a stale
origin for Cancel as well as for Save, so a stale-origin session keeps the
behaviour it has always had rather than being offered a button that only alerts.

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
never in the middle of normal work. When one action ends SEVERAL tabs' sessions
(exit, Disconnect All, Close All), every tab is asked before any session is
resolved (`resolve_pooled_sessions_for_tabs`): the prompt runs a real COMMIT, so
asking tab by tab and acting on each answer as it arrived let a Cancel on the
second tab stop the action with the first tab's transaction already committed
for it. The plan answers with `PooledSessionPlanOutcome`, which keeps a user's
CANCEL apart from a session it could not resolve: both used to be one `false`,
so a failed commit on the third tab aborted an action the first two had already
been committed for AND threw away the answers given for the tabs behind it. A
cancel stops the action; a failure carries every remaining answer out, reports
each one, and lets the action finish — except that a tab whose session could not
be resolved is left OPEN by Close All, because its work is still on that
session. A tab whose query is still running is not part of that plan — its
session belongs to its worker until the query stops, and it is resolved on its
own deferred close. `ScopeChange` is always `Allow` (scope is
applied to the retained session in place and destroys nothing). An `Execute`
that comes back `RequireResolution` no longer pops a modal either
(`resolve_required_transaction_decision`): an `InvalidSession` — the one state
execution cannot proceed on, and one with no user work a commit could reach —
is discarded silently and the statement runs on a fresh session; every other
blocked state keeps the preserved session and lets the statement run on it,
surfacing problems as ordinary statement errors the user can resolve with
Commit/Rollback.

### The session-ending roads resolve their identity lock-free, like the three settings

`SqlEditorWidget::run_pooled_session_close_action` runs the prompt's
Commit/Rollback on the UI thread, on the TAB's own pooled session. It used to
open with `try_lock_connection_with_activity` and answer
`format_connection_busy_message()` — which `apply_pooled_session_resolution`
reads as "refuse the close" — whenever a NEIGHBOUR tab held the connection mutex:
its statement, an Oracle explain plan, an OCI script after `CONNECT`, a metadata
load. None of those has anything to do with this tab's session, and the guard was
released before the take anyway, so it was never providing exclusion either. The
three per-tab pushes were moved off that mutex for exactly this reason; the
prompt the user presses to KEEP their work was a road left on it — and the
toolbar COMMIT/ROLLBACK (`spawn_tracked_transaction_action`) was the other,
found one round later: it held the mutex at TWO gates (the FLTK-thread
operation snapshot and its worker's `try_lock_connection_for_activity`), so the
button answered "Connection is busy" for the same foreign holds, while every
backend released the guard before the wire call.

The toolbar road now confirms the same identity at BOTH ends —
`confirm_retained_session_connection`, the door's identity half, at the plan
and again on the worker just before the take (checked three times in all,
written nowhere) — and carries what the backends used to re-derive from the
guard in the request itself: the exact db type and generation from the
`RetainedSessionTarget`, the `ConnectionInfo` from the confirmed context, the
tab's effective auto-commit and transaction mode resolved at the plan from the
context's connection defaults (constants of the generation, so a cached context
serves them exactly), and the OPERATION's own activity row for the take's
canceler — the row the status bar shows and the registry can cancel, never a
second row of the road's making. Its refusals have one spelling
(`retained_action_refusal_message`): a retired incarnation or a down connection
answers about the SESSION — the loss when the slot held one, its plain absence
when it did not — and only a genuinely busy-or-in-transition connection answers
"busy", through the shared `unreachable_connection_is_gone` classifier both
session-ending roads ask.

One family difference is deliberate and matches the typed statement: the Oracle
action takes the tab's lease directly, so its COMMIT lands while a neighbour
still holds the mutex, while the MySQL family's action goes through the same
per-statement acquire a typed `COMMIT` uses
(`acquire_mysql_pooled_session`, whose startup takes the connection lock), so
it WAITS there exactly as the statement would — bounded by the neighbour's own
operation — and never refuses. `verify_commit_close_live`'s toolbar S-BUSY
scenario asserts both shapes: no alert on any backend, the commit reaching the
server on all four, and on Oracle that it lands during the hold.

It now asks the runtime for the identity
(`ConnectionRuntime::retained_session_target`) and comes through the same door
the pushes use (`begin_retained_session_action`), which resolves the row and the
`ConnectionInfo` from one pool-context read, never blocks, and refuses rather
than carrying on under a blank one. Three answers, each already established
elsewhere:

- a transition in flight (`RuntimeLiveness::InFlight`) is the one answer that is
  not knowledge — the connection may be serving queries again in a moment — so
  the close is REFUSED and the user retries,
- a runtime that is not `Connected`, or a generation the door says has moved
  (`NotThisConnection`), means the incarnation this tab's session was taken under
  is retired: the session is gone, which is reported through
  `retained_session_gone_outcome` and lets the close finish. Refusing there is
  what would leave a down connection's tabs unclosable,
- otherwise the take runs, with an identity that is CHECKED at the door and again
  by the take and written nowhere, so a stale cached generation can only refuse
  this action, never misdirect it.

The door's own `Unreachable` folds TWO of those, and this road is the only caller
that must keep them apart: the connection is BUSY (a neighbour's statement with
nothing in the pool-context cache to fall back on, or a transition announced since
the check above) — refuse; or it cannot be READ AT ALL because it is down — report
and let the tab close. Mapping both to a refusal is exactly the regression the
down-connection arm exists to prevent, so the arm asks the connection itself with
a NON-BLOCKING try-lock, using the same `pool_session_context().is_err()`
predicate that arm always used. It runs only after the door has refused and can
only CLASSIFY, never resolve — there is still one road to the take, which the
guard pins. And the road takes ONE binding snapshot for both the runtime and the
connection: two would let a script `CONNECT` land between them and hand the door
the old connection with the new runtime's identity, which reads as "another
incarnation" and would report a healthy session as lost.

Live: `verify_commit_close_live`'s neighbour-holds-the-mutex scenario, on all
four backends — the close prompt's COMMIT reaches the server while another
thread holds the connection, and the `V + 1` it committed survives a later
`ROLLBACK`.

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
  policy. When BOTH the retained session and the incoming one carry work, the
  tab's existing session is kept and marked `DecisionRequired` — not
  `InvalidSession`. The kept session is live and its `COMMIT` would succeed;
  `InvalidSession` means the server side is gone, which is why it is the one
  state `resolve_required_transaction_decision` discards without asking and
  `capabilities()` never offers commit or rollback for. Marking a live
  work-carrying session with it satisfied the rule the branch was written for
  ("a conflict must not look clean") and cost the user the work anyway.

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
controls until it is fetched out or cancelled, through
`SqlEditorWidget::transaction_mode_change_blocked_now()` — the editor-only view
of the one gate the toolbar asks, since a harness has no window to supply the
rest (S23), and the same gate is what S9 drives for the session half. Finally, two settle the controls' own surface: every isolation level
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
as `SqlAction::ExecuteScript`, and Execute Procedure/Function emits
`SqlAction::OpenInNewTab` — it GENERATES a call script and hands it to an
editor tab, where the user runs it. Each therefore has to obey the tab's
transaction mode and auto-commit like any other statement, which their own
live probes now pin:

- `verify_grid_save_live` — a tab pinned READ ONLY refuses the save and leaves
  the row untouched, unpinning lets the identical save through, and a save on
  an auto-commit tab survives a later `ROLLBACK`. The save also brackets itself
  by what the SESSION holds rather than by the tab's auto-commit flag, so it can
  never commit an explicit transaction of the user's (see
  [grid editing](result_ui.md#grid-editing)).
- `verify_import_live` — a READ ONLY tab refuses the generated import script
  and the target table stays empty; the same script succeeds once unpinned, and
  an import on an auto-commit tab survives a later `ROLLBACK`.
- `verify_proc_exec_live` — a READ ONLY tab refuses a routine that WRITES and
  nothing is written, while a routine that only READS still runs (the pin must
  not over-block), and the same call writes once unpinned; a call on an
  auto-commit tab survives a later `ROLLBACK` too. Its generation round-trip
  additionally pins the script itself on all four backends: the shape follows
  the routine KIND, and every value the routine WRITES has to come back to the
  user — the OUT report (`| OUT: :V_X = ...`) on Oracle, the trailing
  `SELECT @v` on the MySQL family. A local variable cannot show a value once
  the block ends, so an OUT/IN OUT argument is bound whenever a bind can carry
  its type, exactly as the function return value already was.

A routine call and a grid save leave conservative session residue, so those
probes discard the session before pinning auto-commit — on such a session the
menu item itself would be closed, which is a different promise
(`verify_auto_commit_live` S10).

### Pinning a tab is only half of a mode change

`set_tab_transaction_mode()` records the tab's choice; it does not travel to a
physical session the tab is already holding. `update_transaction_mode_from_controls()`
is therefore a three-step path, and all three steps matter:

1. `validate_transaction_option_change()` — refuse the change outright when the
   retained session cannot take it (this is what keeps the screen honest). It
   is told WHICH option is changing as a `TransactionOptionKind`, because two
   of its rules belong to the transaction mode alone and used to be selected by
   comparing the noun the message prints (`action == "transaction mode"`) — one
   reworded string away from taking the wrong branch in silence,
   through `SqlEditorWidget::ensure_retained_session_option_change_allowed`,
   which every backend's step 3 asks as well. Steps 1 and 3 are the same
   question about the same session, so they must not be two rules: the Oracle
   branch used to ask `requires_physical_session_preservation()` instead, and
   the two agreed only because Oracle's statement classifier happens to produce
   a narrower kind of residue than the MySQL one. A step 1 that allows what
   step 3 refuses leaves the toolbar showing a mode the session never got,
   explained by a message that contradicts itself. Only the backend-specific
   part stays dispatched: the MySQL family may REPLACE a pending one-shot on a
   session it would otherwise refuse, because the replacement consumes it.
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
