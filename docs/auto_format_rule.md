# Auto-Formatting Frame and Depth Rules

> Implementation: `src/ui/sql_editor/formatter.rs`
>
> Exhaustive audit: `src/ui/sql_editor/format_sweep_tests.rs`

This document is the normative contract for structural frames and indentation
in SQL auto-formatting. The contract applies equally to Oracle, MySQL, and
MariaDB. Dialects classify syntax differently, but they do not use different
depth rules.

## Terms

- **Owner**: the clause header, opening delimiter, or block header that owns a
  group of direct children.
- **Frame**: the formatter state that records one child `depth`, together with
  the owner's scope, parent, and lifetime.
- **Direct child**: an item immediately contained by a frame. Content inside a
  nested frame is not a direct child of the outer frame.
- **Sibling**: direct children owned by the same frame.
- **Depth**: the indentation level used whenever a token owned by the frame
  starts a rendered line. One depth level is currently four spaces.

## Core contract

### 1. Depth is fixed when the frame is created

If an owner is at depth `d`, its frame stores exactly one child depth,
`d + 1`. This rule also applies when there is only one child and when that
child remains inline. The formatter does not keep separate logical and
physical depth states.

The live frame stack is the single source of structural depth. The formatter
must not add a cached active depth, a previous-frame depth, an `indent_level`
hierarchy, or any equivalent parallel state. A renderer variable that records
the current output cursor is not a structural hierarchy: it may consume a
frame's stored depth when starting a line, but it must not invent a parent-child
edge or add an unowned `+1`/`+2` correction.

Consequently, every visible `d + 2` relationship must be explainable by two
simultaneously live frame edges. For example, a query inside `WITH a AS (` is
one level below the WITH child and one level below the parenthesis owner. If
only one frame edge exists, rendering at `d + 2` is an invariant violation.

A delimiter frame and a syntax-specific typed view may share the same opening
token. They describe one owner edge and therefore store the same `d + 1` child
depth; the typed view is not a second nested owner and must not create `d + 2`.

An inline child does not need an indentation write because no new rendered
line starts there. If that same child continues on another line, every direct
continuation line uses the frame's stored depth. A nested frame uses its own
depth, one level below its owner.

The formatter must not infer a frame's body depth retrospectively from:

- the source indentation;
- the first child after it has been rendered;
- the previous sibling's final line;
- the indentation of a nested child's closing token.

For a sibling-bearing condition or list frame, the first direct item remains
on the owner line and every second or later item starts a new line at the
stored body depth. A later separator must never insert text retrospectively or
move only part of an already-rendered child. Owner modifiers remain part of
the owner header; for example, `WITH RECURSIVE first_cte AS (` keeps
`WITH RECURSIVE` and the first CTE together.

Query, block, and other grammar-boundary frames are not sibling-item lists.
Their body boundary may require the first statement or clause to start on a
new line. For example, the `SELECT` inside `WITH a AS (` starts at `d + 2`:
one live edge for the WITH child and one for the query parenthesis. This does
not permit a sibling-bearing frame to force its first list item onto a new
line.

### 2. Line-start siblings have one body depth

Every direct child that begins a rendered line uses the frame's body depth.
This includes:

- the first child when it begins a line;
- children following `AND` or `OR` in a condition frame;
- children following a comma in a list frame;
- a child following a multiline nested child;
- a leading comment that belongs to the first child.

No direct sibling is allowed to inherit an extra level merely because it is
not the first item.

```sql
WHERE condition_a
    AND condition_b
    OR condition_c
```

The following layout violates the contract because the first child and its
siblings do not share the frame body depth:

```sql
WHERE
condition_a
    AND condition_b
        OR condition_c
```

### 3. First sibling items remain inline

A condition or list frame keeps its first direct sibling item on the owner's
line. The second and later siblings break at `d + 1`, even if the first item is
long or multiline.

```sql
WHERE condition_a
    AND condition_b
SELECT value_a,
    value_b
WITH first_cte AS (
        SELECT 1
    ),
    second_cte AS (
        SELECT 2
    )
```

If a comment makes it impossible to continue on the owner line, the first
line-start child still uses the stored body depth. This is a lexical
necessity, not a different depth rule.

### 4. Nesting composes one frame at a time

A nested owner creates its own body at one level below that owner. Its children
must not be aligned directly from an outer frame or from absolute source
columns.

```sql
WHERE EXISTS (
        SELECT 1
        FROM child_table c
        WHERE c.parent_id = p.id
            AND c.active = 1
    )
    AND p.active = 1
```

When the nested frame ends, subsequent outer siblings return to the outer
frame's body depth.

### 5. Explicit open and close boundaries share owner depth

For parentheses, blocks, and conditional-compilation boundaries, a closing
token that starts a line uses the depth of the owner it closes.

```sql
BEGIN
    process_row (
        first_value,
        second_value
    );
END;
```

This applies to ordinary and semantic parentheses, qualified block endings
such as `END IF` and `END LOOP`, and Oracle conditional compilation `$END`.
An inline close has no independent line-start indentation requirement.

### 6. Original whitespace is not structural evidence

The formatter reconstructs layout from tokens and frame state. Existing spaces
and line breaks may be preserved only by an explicit preservation rule; they
must never establish parentage or body depth.

## Required frame ownership

Every syntax construct with multiple direct children must have exactly one
structural owner for those children. Ownership can be provided by one of these
frame families:

- **Condition frame** for boolean children connected by `AND` or `OR`.
- **List frame** for direct items connected by commas or a clause-specific
  separator.
- **Parenthesized frame** for argument, expression, row, declaration, and other
  delimiter-owned lists.
- **Structural frame** for block statements, query/set branches, CTEs, `MERGE`
  branches, handler/control bodies, `FORALL`, and other non-comma child groups.

The rule is not that every token becomes a frame. A scalar expression, fixed
phrase, or modifier without multiple direct children does not need a child-list
frame.

### Condition ownership

The shared condition rule covers query conditions, control-flow conditions,
loop termination conditions, pattern conditions, and dialect-specific
conditions. Current examples include:

- `WHERE`, `HAVING`, and join `ON`;
- `IF`, `ELSIF`, `WHILE`, and `UNTIL`;
- `START WITH`, `CONNECT BY`, and `QUALIFY`;
- non-CASE `WHEN` and `MATCH_RECOGNIZE ... DEFINE`;
- Oracle conditional compilation `$IF` and `$ELSIF`.

`CASE` branch conditions use the CASE structural frame together with condition
ownership. A nested parenthesized condition creates a nested frame, so its
children may correctly be one level deeper than the surrounding condition.

### List ownership

List frames cover both general and semantic lists, including:

- query projection, source, grouping, ordering, window, assignment, value,
  bind, CTE, and returning lists;
- MODEL, MATCH_RECOGNIZE, PIVOT, JSON_TABLE, and XMLTABLE sections;
- DML target and column groups;
- DDL actions and object groups;
- privileges, grantees, accounts, maintenance targets, and lock targets;
- routine declarations, handlers, diagnostics, and vendor-specific lists.

A comma belongs to the innermost frame whose scope contains the comma. A comma
inside a nested function, row constructor, or subquery must not advance the
outer list.

`FROM` owns its comma-separated relation items. A `JOIN` is not another child
of that comma list: it starts a separate attached join frame at the `FROM`
clause-boundary depth. `ON` or `USING` belongs to the join frame, and boolean
siblings inside `ON` belong to its nested condition frame. Thus `FROM t, u`
indents `u` as the second FROM-list child, while `FROM t JOIN u` aligns `JOIN`
with `FROM` and indents the join body beneath `JOIN`.

For every managed comma list, the first item stays inline with its owner and
the comma before each later item starts that item on the list frame's body
depth. This applies equally to clause lists and delimiter-owned argument,
column, row, and declaration lists; compact input is not an exemption.

## Fixed phrases are not sibling separators

Tokens that spell `AND`, `OR`, `ON`, or a comma are separators only when their
grammar role establishes direct siblings. Fixed phrases and modifiers must not
open or advance a child frame. Important examples include:

- `CREATE OR REPLACE`;
- `BETWEEN ... AND ...`;
- trigger event alternatives such as `INSERT OR UPDATE OR DELETE`;
- function options such as `ON ERROR` and `ON EMPTY`;
- multiword clause headers and dialect modifiers.

Classification must use dialect and structural context, not the token text
alone.

## Comment and multiline-token rules

- A leading comment attached to a child uses that child's body depth.
- A trailing inline comment remains attached to its rendered token.
- Keywords and delimiters inside comments or quoted literals never create,
  advance, or close frames.
- A multiline string or comment must not leak indentation state into the next
  sibling.

## Canonical-output requirements

Frame-correct output must also satisfy the general formatter contract:

- token content and order are preserved;
- a second formatting pass produces byte-identical output;
- no trailing whitespace is introduced;
- dialect-specific compact syntax remains compact where required;
- frame state does not survive outside its scope or statement lifetime.

## Automated audit requirements

`formatting_sweep_all_files_generate_out_report` must fail when it detects any
of the following:

- a duplicate frame ID;
- a missing or non-containing parent;
- a close without an opener, a close before its opener, or duplicate closes;
- an explicit frame without a close;
- a child frame closing after its parent;
- a line-start first child or sibling at a depth different from its frame body;
- a frame whose stored depth is not its owner depth plus one, including a
  single inline child;
- a sibling-bearing condition/list frame whose first item is moved to a new
  line without a forcing comment;
- an `AND`/`OR` or comma-connected sibling without an owning frame;
- a line-start close at a depth different from its opener/owner;
- body-depth drift after a nested frame or leading comment;
- a parallel structural-depth cache or output-only indentation hierarchy;
- a production managed-frame or typed-list kind missing from the independent
  syntax inventory.

The complete sweep covers every `.sql` and `.txt` fixture under `test/`,
`test_mysql/`, and `test_mariadb/`.

## Adding or changing syntax

Before merging formatter support for a construct with multiple children:

1. Identify the grammatical owner and direct-child boundary.
2. Reuse an existing frame family or add a typed frame kind.
3. Set body depth from the owner when the frame is created.
4. Define the exact close or scope-expiration boundary.
5. Exclude fixed phrases that reuse separator tokens.
6. Test the first child inline and at line start.
7. Test at least two siblings, a multiline nested child, and a following outer
   sibling.
8. Test leading/trailing comments and quoted separator text.
9. Add the construct to the independent frame inventory.
10. Run the focused invariants, the complete fixture sweep, and the full Rust
    quality gates.

## Verification commands

```sh
cargo test --lib formatting_sweep_
cargo test --lib formatting_sweep_all_files_generate_out_report -- --ignored --nocapture
cargo test
cargo clippy --locked --all-targets -- -D warnings -W clippy::perf -W clippy::complexity
cargo fmt --all -- --check
```
