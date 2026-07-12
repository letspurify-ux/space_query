# SQL Syntax Highlighting

> Implementation: `src/ui/syntax_highlight.rs`,
> `src/ui/sql_editor/highlighting.rs`, `src/ui/sql_editor/chunked_text.rs`

Syntax highlighting is a stateful lexer informed by database dialect and
metadata, not a full SQL parser. It selects colors; separate modules determine
IntelliSense context and candidates.

## Components

- `SqlHighlighter` classifies tokens and transitions `LexerState`.
- `HighlightData` supplies object and column names loaded from the schema.
- `HighlightShadowState` stores editor text, logical styles, and per-line exit
  states in chunks.
- `src/ui/sql_editor/highlighting.rs` owns full/incremental rehighlighting and
  FLTK style-buffer synchronization.

`HighlightData` includes tables, views, columns, materialized views, routines,
packages, sequences, triggers/events, types, indexes, synonyms, and schemas.
Relation and column lookups are rebuilt with uppercase keys.

## Styles

Logical styles assign one ASCII tag to every byte of SQL text.

| Tag | Meaning |
| --- | --- |
| `A` | Default text |
| `B` | Keyword |
| `C` | Built-in function or known built-in |
| `D` | Single-quoted string |
| `E` | Line comment or closed regular block comment |
| `F` | Number |
| `G` | Operator |
| `H` | Schema-object identifier |
| `I` | Hint comment |
| `J` | Datetime/interval literal |
| `K` | Column |
| `L` | Multiline block-comment continuation |
| `M` | Oracle q-quote string |
| `N` | Quoted identifier |

Before writing to FLTK, `encode_fltk_style_bytes()` keeps the tag on the first
byte of a UTF-8 character and replaces continuation bytes with `0`. Text and
style buffers must always have identical byte lengths.

## Lexer state

`LexerState` carries these states between lines:

- Normal
- Block or hint comment
- Single quote
- Q-quote closing delimiter and nesting depth
- Double-quoted identifier
- MySQL/MariaDB backtick identifier

Multiline continuation comes from per-line `line_exit_states`, not by reversing
a style tag. This is essential for q-quotes, whose closing delimiter cannot be
recovered from tag `M` alone.

## Token classification

For each line, the highlighter first consumes the incoming state and then
classifies roughly in this order:

1. Line-leading commands such as `PROMPT` and `CONNECT`
2. Line, block, and hint comments
3. Q-quotes and prefixed/single-quoted strings
4. Double quotes and MySQL-compatible backtick identifiers
5. Numbers
6. Keywords, built-ins, schema objects, columns, and ordinary identifiers
7. Operators

Keyword and built-in catalogs depend on the current `DatabaseType`. MySQL and
MariaDB share MySQL-compatible quote/comment handling and function catalogs.
Relation, alias, and member-access context is adjusted lexically but does not
provide AST-level semantic proof.

## Full rehighlighting

`rehighlight_full_buffer()` scans the complete text line by line. Each line
receives the previous line's exit state and produces logical styles plus a new
exit state. The full style sequence is then encoded for FLTK, and the shadow and
style buffer are replaced together.

This path is used after metadata/database-type changes and whenever incremental
state cannot be recovered safely.

## Incremental rehighlighting

The buffer modify callback follows this sequence:

1. Apply the insertion/deletion to shadow text and placeholder styles.
2. Rescan from the start of the modified line.
3. Always recalculate through the line containing the end of the modified span.
4. Continue until both styles and exit state match the old shadow.
5. Write only the smallest changed style range to FLTK.

Every position is a UTF-8 byte offset aligned through `ChunkedText` boundary and
line helpers. Style-length mismatch or shadow-update failure falls back to a
full rehighlight.

## IntelliSense boundary

Highlighting and IntelliSense share object metadata but have distinct owners:

- Lexical state and color: `syntax_highlight.rs`, `highlighting.rs`
- SQL phase and cursor context: `src/ui/intellisense_context.rs`
- Candidate collection/filtering: `src/ui/sql_editor/intellisense/`

`HighlightShadowState::cursor_in_string_or_comment()` suppresses completion in
single/q-quote/datetime strings and ordinary line/block comments using existing
styles. It does not choose candidate types. Quoted identifiers and hint styles
are not part of that suppression set.

## Verification

```sh
cargo test syntax_highlight --lib
cargo test incremental_highlighting --lib
cargo test encode_fltk_style_bytes --lib
cargo test cursor_in_string_or_comment --lib
```

When adding a multiline lexical construct, test full/incremental equivalence,
cross-line state propagation, and UTF-8 byte-length preservation.
