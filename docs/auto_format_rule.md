# Auto-Formatting Frame Rules

> Implementation: `src/ui/sql_editor/formatter.rs`
>
> Sweep audit: `src/ui/sql_editor/format_sweep_tests.rs`

This document defines the structural rules for SQL auto-formatting. Oracle,
MySQL, and MariaDB may classify syntax differently, but use the same frame and
depth rules.

## 1. Frames are the only source of depth

A frame represents one grammatical owner and its direct children.

- An owner at depth `d` creates its child frame at `d + 1`.
- The frame has the same depth even when it has only one child.
- An inline child also has the frame depth, although no indentation is written
  until that child starts or continues on a new line.
- Nested syntax adds one depth only when it creates a real nested frame.
- Every visible `d + 2` must therefore be explained by two live frame edges.

Do not maintain a separate logical depth, `indent_level`, cached structural
depth, or output-only correction. The live frame stack is the structural source
of truth.

## 2. Parentage follows grammar, not output order

A frame is chosen from the grammatical owner/direct-child relationship. A
construct is not a sibling merely because it appears after another construct.

```sql
FROM table_a
JOIN table_b
    ON table_b.id = table_a.id
        AND table_b.active = 1
```

Here `table_a` is a child of the FROM list. `JOIN` is a separate attached frame
at the FROM clause-boundary depth, `ON` belongs to the JOIN frame, and `AND`
belongs to the nested ON condition frame.

By contrast, the comma below creates FROM-list siblings:

```sql
FROM table_a,
    table_b
```

This ownership rule also applies to attached constructs such as `APPLY`,
`PIVOT`, and `UNPIVOT`. Each construct must be classified by grammar before its
depth is calculated.

## 3. Depth comes from the parent frame, not the rendered line

An owner may start in the middle of a rendered line:

```sql
SELECT SUM(CASE
            WHEN active = 1 THEN amount
            ELSE 0
        END
    )
```

The structural depth of `SUM` or `CASE` is not the number of spaces at the
start of the `SELECT` line. A new frame derives its depth from its direct parent
frame. Line indentation is only the visible result when a token starts a new
line.

Never derive structural depth from:

- source indentation or source line breaks;
- the leading spaces of a line containing an inline owner;
- a previously rendered sibling;
- a nested child's closing line.

## 4. Sibling frames keep the first child inline

For list and condition frames:

- the first direct child remains on the owner line;
- the second and later children start new lines at the frame depth;
- all direct children that start lines use the same depth.

```sql
SELECT value_a,
    value_b
FROM table_a,
    table_b
WHERE condition_a
    AND condition_b
    OR condition_c
```

The rule applies to comma lists, function arguments, row values, CTEs,
declarations, assignments, and boolean conditions. A nested comma or boolean
operator advances only its innermost owning frame.

A later separator must not retrospectively move or partially rewrite the first
child.

## 5. Structural body boundaries are separate frames

The first-child-inline rule applies to sibling-bearing list and condition
frames. It does not remove grammar-required body boundaries.

`BEGIN`, `THEN`, query bodies, loop bodies, exception handlers, and similar
constructs may start their first body statement on a new line because they open
a structural frame.

```sql
WHEN condition THEN
    process_row();
```

If that body begins with a parenthesized expression, the first child of the
parenthesis still remains inline:

```sql
WHEN condition THEN
    result := (CASE
            WHEN flag = 1 THEN value_a
            ELSE value_b
        END);
```

## 6. Nesting and closing boundaries

Each real nested owner adds exactly one frame. When it closes, following outer
siblings return to the outer frame depth.

A closing delimiter or block ending that starts a line uses the depth of the
owner it closes.

```sql
WITH first_cte AS (
        SELECT value_a,
            value_b
        FROM source_table
    ),
    second_cte AS (
        SELECT value_a
        FROM first_cte
    )
SELECT value_a
FROM second_cte;
```

In `WITH first_cte AS (`, the CTE is the inline first WITH child. The nested
`SELECT` crosses the WITH-child and parenthesis edges, so it is `d + 2`. The
closing `)` returns to the CTE child depth, `d + 1`.

A syntax-specific typed view and a delimiter frame may describe the same owner
edge. They must share one depth rather than double-counting it.

## 7. Comments and preserved text do not change structure

- A comment that forces the first child onto a new line uses the child's frame
  depth.
- A trailing comment stays attached to its token.
- Keywords and separators inside comments or quoted literals do not affect
  frames.
- Multiline comments and strings must not leak depth into following syntax.
- Preserved source whitespace is never evidence of parentage or depth.

## 8. Separators are grammatical, not textual

`AND`, `OR`, `ON`, and commas act as sibling separators only when their grammar
role creates direct children. They must not split fixed phrases or modifiers,
including:

- `CREATE OR REPLACE`;
- `BETWEEN ... AND ...`;
- trigger events such as `INSERT OR UPDATE OR DELETE`;
- `ON ERROR`, `ON EMPTY`, and other multiword options.

## 9. Frame ownership coverage

Every construct with multiple direct children must have one explicit owner:

- condition frames for boolean children;
- list frames for comma or clause-specific siblings;
- parenthesis frames for delimiter-owned children;
- structural frames for query, block, CASE, CTE, MERGE, handler, loop, and
  similar bodies;
- attached frames for constructs such as JOIN that share a clause boundary but
  own their own body.

Scalar expressions and fixed phrases without child groups do not need frames.

## 10. Canonical output

Frame-correct formatting must also:

- preserve token content and order;
- produce identical output on a second formatting pass;
- introduce no trailing whitespace;
- keep dialect-specific compact syntax where required;
- close or expire every frame within its grammatical scope.
