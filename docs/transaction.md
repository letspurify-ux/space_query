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
```

When adding a SQL family, test classification, implicit commit,
transaction/residue/lock effects, and cleanup-only preflight. Run the
[session verification](session.md#verification) for interruption and concurrency
regressions.
