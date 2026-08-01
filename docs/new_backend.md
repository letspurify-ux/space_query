# Adding a Database Backend

> Implementation: `src/db/connection.rs`, `src/db/query/`,
> `src/db/sql_classification.rs`, `src/db/transaction.rs`,
> `tests/db_dispatch_guards.rs`

First decide whether the database shares an existing backend family and SQL
dialect. MariaDB has a separate `DatabaseType` but reuses
`DatabaseBackendKind::MySql` and `SqlDialect::MySql`. A database with different
protocol, transaction semantics, and syntax needs variants in all three enums.

## 1. Add dispatch variants

Add the `DatabaseType` variant and update `DatabaseType::ALL`. For a new family,
also extend `DatabaseBackendKind` and `SqlDialect`. Assign a unique cache key and
preserve the `from_cache_key()` round trip.

The project uses exhaustive `match` expressions so missing dispatch appears as
a compiler error. `tests/db_dispatch_guards.rs` also rejects ad hoc comparisons
to concrete database types, wildcard dispatch, anonymous physical-session
discard, and driver-error markers embedded in generic UI code. Run these first
to enumerate required decisions:

```sh
cargo check --all-targets
cargo test --test db_dispatch_guards
```

## 2. Implement connection policy

Implement `DbBackend` and register it in `backend_for()`. Required methods cover:

- Connection form, defaults, advanced settings, and validation
- Primary connection and pool construction, including pool-session setup
- Current schema/database lookup, switch, and reapplication
- SSL, timeout, and auto-commit
- Transaction isolation/access mode and first-statement constraints
- Conditions that block retained-session option or scope changes
- Cache key and UI labels

State intentional no-ops in the backend implementation. Do not hide new policy
behind a trait default. New persisted fields in `ConnectionAdvancedSettings`
need `#[serde(default)]` so existing configurations still deserialize.

## 3. Implement execution and classification

Connect the new type to these registries and traits:

| Responsibility | Location |
| --- | --- |
| Statement result/timeout profile | `src/db/query/execution_backend.rs` |
| SQL kind and comment/dialect profile | `src/db/sql_classification.rs` |
| Query/script executor | `src/db/query/executor.rs` or a new executor |
| UI worker entry | `src/ui/sql_editor/execution.rs` |
| Commit/rollback/discard and Explain Plan | `src/ui/sql_editor/mod.rs` |

`SqlKind` and `StatementSessionEffects` affect both result routing and session
safety after interruption. Decide and test:

- Select-like and result-set statements
- DML, DDL, and implicit commit
- Transaction and session control
- Procedure, script, and unknown statements
- Session residue such as temporary objects, prepared state, variables, locks
- Statements that modify the same timeout setting used by the UI

Never downgrade uncertain SQL to a safe SELECT. Follow the
[session](session.md) and [transaction](transaction.md) contracts.

## 4. Register UI behavior

Register concrete `DatabaseType` behavior explicitly:

- Schema metadata loader: `src/ui/main_window.rs`
- Object-browser behavior: `src/ui/object_browser.rs`
- Keyword/function catalogs: `src/ui/intellisense.rs`,
  `src/ui/syntax_highlight.rs`
- Quick describe, signatures, and column loader:
  `src/ui/sql_editor/intellisense/`
- Formatter dialect fallback: `src/sql_text.rs`

Even when an existing family is reused, every registry must state which
implementation handles the new concrete type. Do not scatter concrete database
comparisons through ordinary UI widgets.

## 5. Results and grid editing

Executors use the shared `QueryResult`, `QueryProgress`, and `result_messages`
contracts. Lazy fetch must implement cursor cleanup, cancellation, fetch-all
timeout, and retained-lease return.

Grid editing requires injecting a stable row identifier and routing save-DML
results back to the originating edit tab. If unsupported, do not inject an
identifier; the edit action will remain hidden. Oracle's `SQ_INTERNAL_ROWID`
flow is an example, not a universal backend contract.

Register driver cancel, abort, and connection-loss text in the database-specific
marker catalogs in `src/db/session_policy.rs`. Never place those strings in
generic result or query-history UI code.

## 6. Completion criteria

- [ ] Every dispatch `match` and `DatabaseType::ALL` is updated.
- [ ] Primary, pool, and retained sessions share setting and scope policy.
- [ ] SQL kind, implicit commit, and residue/lock effects are tested.
- [ ] Cancellation, timeout, lazy-fetch close, and stale generation are tested.
- [ ] Commit/rollback/discard cannot bypass central preflight.
- [ ] Result routing and grid-edit support decisions are verified.
- [ ] Keyword, highlighting, IntelliSense, and formatter dialects are registered.
- [ ] Cache key and configuration migration preserve existing profiles.

Minimum verification:

```sh
cargo fmt --check
cargo check --all-targets
cargo test --lib
cargo test --test db_dispatch_guards
```

If the backend changes session lifecycle behavior, also run the
[session verification](session.md#verification).
