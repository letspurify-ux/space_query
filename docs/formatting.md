# SQL Auto-Formatting

> Implementation: `src/ui/sql_editor/formatter.rs`,
> `src/db/query/script.rs`, `src/sql_text.rs`, `src/sql_format.rs`,
> `src/ui/sql_depth.rs`

Auto-formatting rebuilds a canonical layout from tokens and structural state; it
does not merely adjust existing whitespace. Indentation is currently fixed at
four spaces.

The normative frame ownership, child-depth, and close-boundary contract is
defined in [Auto-Formatting Frame and Depth Rules](auto_format_rule.md).

## Runtime flow

1. The script/parser layer separates SQL and tool commands into `FormatItem`
   values.
2. The formatter resolves database type/dialect and tokenizes the input.
3. `FormatFrameStack` applies query, delimiter, block, condition-owner, and
   scoped state in token order.
4. The renderer emits keyword casing, spacing, line breaks, indentation, and
   terminators.
5. Selection formatting remaps cursor and selection positions to the result.

`QueryExecutor::auto_format_line_contexts()` is a parallel analyzer and test
surface, not a function called by the runtime formatter. Its
`AutoFormatLineContext` records parser/auto/render/carry depth, query role, line
semantics, query base, and condition-header information. The runtime formatter
uses its own stack; both paths share classifiers from `sql_text.rs` and
`sql_format.rs`.

## Structural invariants

### Depth comes from structure

- Existing indentation is not evidence for structural depth.
- One opener creates one frame and adds one level.
- A same-line close/open chain such as `) + (` is processed in token order, not
  as a net delta.
- Leading closes are consumed before classifying the remaining tail as a clause
  or owner.
- A close line aligns to the delimiter or block owner it actually closes, not
  to the preceding line.

`src/ui/sql_depth.rs` and `src/sql_delimiter.rs` share delimiter-stack and
pre-token depth calculations. Delimiters inside comments or quoted literals are
not structural events.

### Deferred state preserves meaning

Structures completed on a later line are represented by named pending/frame
state, never anonymous previous-line shape. This includes:

- Split owners, headers, and query heads
- Condition headers and body terminators
- `MERGE WHEN ... THEN` branches
- Multiline clause owners
- PL/SQL and MySQL compound blocks with qualified `END`
- CTE and cursor/open-for child queries
- Operator RHS and inline-comment continuation

Pending state may carry query and owner frame IDs. `SqlFormatFrameContext`
prevents state from leaking into a sibling frame at the same depth.

### Stack lifetime is LIFO

The runtime renderer's structural authority is `FormatFrameStack::frames`.
Paren, block, query, condition, and auxiliary frames are pushed and popped at
the tail. The `*_frame_indices` fields are lookup caches, not separate owners.
Stack changes must restore caches and runtime state together.

## Authoritative classifiers

Long keyword and owner lists are intentionally not duplicated here.

| Classification | Authority |
| --- | --- |
| Oracle/MySQL keywords and statement heads | `src/sql_text.rs` |
| Query, multiline, and PL/SQL owners | `FormatQueryOwnerKind`, `FormatIndentedParenOwnerKind`, and helpers |
| Split body/header continuation | `FormatBodyHeaderContinuationState` and matcher |
| Operator RHS | `FormatTrailingContinuationOperatorKind` |
| Inline-comment header carry | `FormatInlineCommentHeaderContinuationKind` |
| Analyzer line context | `QueryExecutor::auto_format_line_contexts()` |
| Renderer frame transitions | `FormatFrameStack` and `format_statement()` |

Do not copy a new syntax as literal-string exceptions across several phases.
Extend the shared lexical classifier first, then make analyzer and renderer
consume the same semantic family.

## Dialect and preservation rules

- Oracle and MySQL/MariaDB have different keyword, command, and delimiter
  policy.
- MySQL/MariaDB function calls, type precision, and routine calls keep tight
  parentheses.
- A token spelled like a keyword preserves source casing when its context makes
  it an identifier or alias.
- SQL*Plus `PROMPT`, remarks, and verbatim items use dedicated preservation
  paths.
- Inline comments and multiline literals must not change owner, close, or
  continuation recognition.
- The formatter does not invent a semicolon for an incomplete final statement.

## Canonical output

The same tokens and structural state must converge to the same output regardless
of input whitespace. Formatting the result again must leave it unchanged. The
highest-risk idempotence boundaries include:

- Nested subqueries/CTEs and mixed close-open lines
- `CASE`, PL/SQL, and MySQL compound blocks
- `MERGE`, `MODEL`, `MATCH_RECOGNIZE`, `PIVOT/UNPIVOT`, and `WINDOW`
- JSON/XML function options and owner-relative clauses
- Comments between an owner, header, or operator and its continuation

## Adding syntax

1. Extend the lexical/semantic family in `sql_text.rs`.
2. Add the analyzer transition for `AutoFormatLineContext`.
3. Add the renderer transition for `FormatFrameStack`.
4. Test a minimal case, nested/sibling cases, and comment/quoted-literal cases.
5. Format the result twice and assert equality.

## Verification

```sh
cargo test formatter --lib
cargo test auto_format_line_contexts --lib
cargo test sql_format --lib
cargo test sql_depth --lib
```

Large regression inputs live under `test/` with `.out` fixtures. If new syntax
is unrelated to an existing fixture, add a focused test instead of rewriting
the entire fixture.
