# Developer Documentation

This directory documents the current implementation. Start with the repository
[README](../README.md) for the user-facing feature overview and run instructions.

## Reading order

| Area | Document | Scope |
| --- | --- | --- |
| Editor | [Syntax highlighting](highlighting.md) | Lexer, incremental rehighlighting, UTF-8/style buffer |
| Editor | [Auto-format rules](auto_format_rule.md) | Normative frame ownership, child depth, close, and audit contract |
| Editor | [SQL formatting](formatting.md) | Structural depth, formatter state, verification principles |
| Results | [Result UI](result_ui.md) | Result tabs, support panes, selection and close rules |
| Sessions | [Session lifecycle](session.md) | Execution, cancellation, timeout, lazy fetch, physical-session decisions |
| Transactions | [Retained sessions](transaction.md) | Transactions, residue, locks, preflight, user resolution |
| Connections | [Oracle](oracle.md) | Thin/OCI connections and Oracle live tests |
| Connections | [MySQL](mysql.md) | MySQL connection settings and live tests |
| Connections | [MariaDB](mariadb.md) | Shared MySQL-family behavior and MariaDB differences |
| Extension | [Adding a backend](new_backend.md) | Adding a `DatabaseType` or backend family |

## Document boundaries

- `session.md` alone owns the rules for reusing a physical session after an
  interrupted operation.
- `transaction.md` alone owns transaction, session-residue, lock, and preflight
  state.
- `mysql.md` describes shared MySQL/MariaDB connection behavior;
  `mariadb.md` contains only genuine MariaDB differences.
- Source code is authoritative for formatter and highlighter keyword catalogs.
  The documents identify the classifiers and invariants instead of copying long
  lists.
- Local test container coordinates and development-only credentials documented
  by each database guide are fixtures, not implementation contracts. Never
  record production credentials here.

## Verification standard

When changing a document, inspect the source files and tests named by that
document. Public type names, enum variants, and commands must be searchable in
the current worktree. Tests marked `#[ignore]` require an external database and
must be run explicitly with the documented environment variables.
