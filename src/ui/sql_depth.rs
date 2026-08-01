use crate::sql_delimiter::{
    line_closes_delimiter_frame_below_snapshot_before_token, line_start_snapshot_before_token,
    DelimiterFrameState,
};
use crate::ui::sql_editor::SqlToken;

pub(crate) type ParenDepthState = DelimiterFrameState;

/// Returns true when the token stream between `line_start_idx` and `token_idx`
/// closes at least one delimiter frame that existed at line start depth before
/// `token_idx` is reached.
///
/// This preserves strict token-order semantics for close/open chains such as
/// `) + (` and `] + [`, using an explicit frame stack instead of net delta
/// counting.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn line_closes_delimiter_frame_below_depth_before_token(
    tokens: &[SqlToken],
    line_start_idx: usize,
    token_idx: usize,
    line_start_depth: usize,
) -> bool {
    let line_start_snapshot =
        line_start_snapshot_before_token(tokens, line_start_idx, line_start_depth);
    line_closes_delimiter_frame_below_snapshot_before_token(
        tokens,
        line_start_idx,
        token_idx,
        &line_start_snapshot,
    )
}

/// Returns the parenthesis depth *before* each token is processed.
///
/// Depth changes for grouping symbols (`()`, `[]`, `{}`) and never goes below zero.
pub(crate) fn paren_depths(tokens: &[SqlToken]) -> Vec<usize> {
    let mut depths = Vec::with_capacity(tokens.len());
    let mut state = ParenDepthState::default();

    for token in tokens {
        depths.push(state.depth());
        state.apply_token(token);
    }

    depths
}

/// Applies parenthesis depth transition for a single token.
#[inline]
pub(crate) fn apply_paren_token(state: &mut ParenDepthState, token: &SqlToken) {
    state.apply_token(token);
}

/// Returns the final parenthesis depth after all tokens are processed.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn paren_depth_after(tokens: &[SqlToken]) -> usize {
    let mut state = ParenDepthState::default();
    for token in tokens {
        state.apply_token(token);
    }
    state.depth()
}

/// Returns token depth at `idx`, treating out-of-range indices as depth 0.
#[inline]
pub(crate) fn depth_at(depths: &[usize], idx: usize) -> usize {
    depths.get(idx).copied().unwrap_or(0)
}

/// Returns true when a token is at top-level (depth 0).
#[inline]
pub(crate) fn is_top_level_depth(depths: &[usize], idx: usize) -> bool {
    depth_at(depths, idx) == 0
}

/// Returns true when a token is at a specific depth.
#[inline]
#[allow(dead_code)]
pub(crate) fn is_depth(depths: &[usize], idx: usize, depth: usize) -> bool {
    depth_at(depths, idx) == depth
}

/// Splits tokens by a top-level delimiter symbol (예: `,`) while ignoring nested
/// parenthesis depth.
///
/// Empty segments are skipped to match existing parser behavior.
pub(crate) fn split_top_level_symbol_groups<'a>(
    tokens: &'a [SqlToken],
    delimiter: &str,
) -> Vec<Vec<&'a SqlToken>> {
    let mut groups: Vec<Vec<&'a SqlToken>> = Vec::new();
    let mut current: Vec<&'a SqlToken> = Vec::new();
    let mut state = ParenDepthState::default();

    for token in tokens {
        let at_root = state.depth() == 0;
        if let SqlToken::Symbol(sym) = token {
            if sym == delimiter && at_root {
                if !current.is_empty() {
                    groups.push(std::mem::take(&mut current));
                }
                continue;
            }
        }

        current.push(token);
        state.apply_token(token);
    }

    if !current.is_empty() {
        groups.push(current);
    }

    groups
}

/// Splits tokens by top-level SQL keyword boundaries while preserving the keyword
/// as the first token of the next group.
///
/// Empty segments are skipped to match existing parser behavior.
pub(crate) fn split_top_level_keyword_groups<'a>(
    tokens: &'a [SqlToken],
    break_keywords: &[&str],
) -> Vec<Vec<&'a SqlToken>> {
    let mut groups: Vec<Vec<&'a SqlToken>> = Vec::new();
    let mut current: Vec<&'a SqlToken> = Vec::new();
    let mut state = ParenDepthState::default();

    for token in tokens {
        let is_break = match token {
            SqlToken::Word(word) => {
                state.depth() == 0
                    && break_keywords
                        .iter()
                        .any(|keyword| word.eq_ignore_ascii_case(keyword))
            }
            _ => false,
        };

        if is_break && !current.is_empty() {
            groups.push(std::mem::take(&mut current));
        }

        current.push(token);
        state.apply_token(token);
    }

    if !current.is_empty() {
        groups.push(current);
    }

    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::sql_editor::query_text::tokenize_sql;

    fn sym(s: &str) -> SqlToken {
        SqlToken::Symbol(s.to_string())
    }
    fn word(s: &str) -> SqlToken {
        SqlToken::Word(s.to_string())
    }

    // ── paren_depths ──────────────────────────────────────────────────────────

    #[test]
    fn paren_depths_empty_returns_empty() {
        assert_eq!(paren_depths(&[]), Vec::<usize>::new());
    }

    #[test]
    fn paren_depths_records_pre_token_depth() {
        // For tokens: word, (, word, ), word
        // depths before each:  0,  0,  1,  1,  0
        let tokens = [word("a"), sym("("), word("b"), sym(")"), word("c")];
        assert_eq!(paren_depths(&tokens), vec![0, 0, 1, 1, 0]);
    }

    #[test]
    fn paren_depths_nested_parens() {
        // ((x)) → depths before each token: (, (, x, ), )
        //                                    0  1  2  2  1
        let tokens = [sym("("), sym("("), word("x"), sym(")"), sym(")")];
        assert_eq!(paren_depths(&tokens), vec![0, 1, 2, 2, 1]);
    }

    #[test]
    fn paren_depths_tracks_brackets_and_braces() {
        let tokens = [sym("["), sym("{"), word("x"), sym("}"), sym("]"), word("y")];
        assert_eq!(paren_depths(&tokens), vec![0, 1, 2, 2, 1, 0]);
    }

    #[test]
    fn paren_depths_handles_multiple_group_symbols_in_one_token() {
        // Some token streams may include grouped symbols in a single token (e.g. "((").
        // Depth must reflect every symbol, not just the first char.
        let tokens = [sym("(("), word("x"), sym("))")];
        assert_eq!(paren_depths(&tokens), vec![0, 2, 2]);
        assert_eq!(paren_depth_after(&tokens), 0);
    }

    #[test]
    fn paren_depths_saturates_at_zero_for_unbalanced_close() {
        // ) at depth 0 must not underflow
        let tokens = [sym(")"), word("x")];
        assert_eq!(paren_depths(&tokens), vec![0, 0]);
    }

    #[test]
    fn paren_depths_ignores_string_and_comment_tokens() {
        // Parens inside string/comment variants must not affect depth
        let tokens = [
            SqlToken::String("(text)".to_string()),
            SqlToken::Comment("-- (note)".to_string()),
            word("x"),
        ];
        assert_eq!(paren_depths(&tokens), vec![0, 0, 0]);
    }

    fn comma_index(tokens: &[SqlToken]) -> usize {
        tokens
            .iter()
            .enumerate()
            .find(|(_, token)| matches!(token, SqlToken::Symbol(sym) if sym == ","))
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    }

    // ── line_closes_delimiter_frame_below_depth_before_token ───────────────

    #[test]
    fn line_close_detection_tracks_close_then_open_token_order() {
        let tokens = tokenize_sql(") + (, stable");
        let comma_idx = comma_index(&tokens);

        assert!(
            line_closes_delimiter_frame_below_depth_before_token(&tokens, 0, comma_idx, 1),
            "close->open sequence must report a frame close below the line-start depth before comma"
        );
    }

    #[test]
    fn line_close_detection_ignores_local_balanced_parens() {
        let tokens = tokenize_sql("ABS(a), stable");
        let comma_idx = comma_index(&tokens);

        assert!(
            !line_closes_delimiter_frame_below_depth_before_token(&tokens, 0, comma_idx, 0),
            "locally balanced call parens must not be treated as closing a parent frame"
        );
    }

    #[test]
    fn line_close_detection_does_not_report_parent_close_when_line_start_depth_is_zero() {
        let tokens = tokenize_sql(") + (, stable");
        let comma_idx = comma_index(&tokens);

        assert!(
            !line_closes_delimiter_frame_below_depth_before_token(&tokens, 0, comma_idx, 0),
            "line-start depth 0 has no parent frame to close; unmatched close must not trigger close-detection"
        );
    }

    #[test]
    fn line_close_detection_does_not_report_parent_close_for_brackets_at_depth_zero() {
        let tokens = tokenize_sql("] + [, stable");
        let comma_idx = comma_index(&tokens);

        assert!(
            !line_closes_delimiter_frame_below_depth_before_token(&tokens, 0, comma_idx, 0),
            "line-start depth 0 has no parent frame to close even for bracket close/open sequences"
        );
    }

    #[test]
    fn line_close_detection_tracks_close_then_open_for_brackets() {
        let tokens = tokenize_sql("] + [, stable");
        let comma_idx = comma_index(&tokens);

        assert!(
            line_closes_delimiter_frame_below_depth_before_token(&tokens, 0, comma_idx, 1),
            "close->open bracket sequence must report a frame close below the line-start depth before comma"
        );
    }

    #[test]
    fn line_close_detection_respects_typed_line_start_frame_kind() {
        let tokens = tokenize_sql("(\n] + (, stable");
        let line_start_idx = tokens
            .iter()
            .enumerate()
            .find(|(_, token)| matches!(token, SqlToken::Symbol(sym) if sym == "]"))
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        let comma_idx = comma_index(&tokens);

        assert!(
            !line_closes_delimiter_frame_below_depth_before_token(
                &tokens,
                line_start_idx,
                comma_idx,
                1,
            ),
            "known line-start `(` frame must not be popped by mismatched `]` before comma"
        );
    }

    #[test]
    fn line_close_detection_tracks_typed_line_start_close_then_open() {
        let tokens = tokenize_sql("(\n) + (, stable");
        let line_start_idx = tokens
            .iter()
            .enumerate()
            .find(|(_, token)| matches!(token, SqlToken::Symbol(sym) if sym == ")"))
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        let comma_idx = comma_index(&tokens);

        assert!(
            line_closes_delimiter_frame_below_depth_before_token(
                &tokens,
                line_start_idx,
                comma_idx,
                1,
            ),
            "known line-start `(` frame should be consumed before a later `(` opens again"
        );
    }

    #[test]
    fn line_close_detection_respects_explicit_line_start_depth_when_visible_stack_is_deeper() {
        let tokens = tokenize_sql("((\n) + value, tail");
        let line_start_idx = tokens
            .iter()
            .enumerate()
            .find(|(_, token)| matches!(token, SqlToken::Symbol(sym) if sym == ")"))
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        let comma_idx = comma_index(&tokens);

        assert!(
            !line_closes_delimiter_frame_below_depth_before_token(
                &tokens,
                line_start_idx,
                comma_idx,
                1,
            ),
            "line-start depth 1 should not report a close when the stream only drops from visible depth 2 to 1 before comma"
        );
        assert!(
            line_closes_delimiter_frame_below_depth_before_token(
                &tokens,
                line_start_idx,
                comma_idx,
                2,
            ),
            "line-start depth 2 should report the same close because it drops below the explicit line-start frame depth"
        );
    }

    #[test]
    fn line_close_detection_ignores_mismatched_local_close_delimiter() {
        let tokens = tokenize_sql("( ] , stable");
        let comma_idx = comma_index(&tokens);

        assert!(
            !line_closes_delimiter_frame_below_depth_before_token(&tokens, 0, comma_idx, 1),
            "mismatched close inside a local frame must not be treated as closing a parent frame"
        );
    }

    #[test]
    fn line_close_detection_tracks_later_close_inside_single_grouped_symbol_token() {
        let tokens = [sym("())("), sym(","), word("stable")];

        assert!(
            line_closes_delimiter_frame_below_depth_before_token(&tokens, 0, 1, 2),
            "grouped symbol token `())(` should still detect the later close that drops below the line-start frame depth"
        );
    }

    // ── paren_depth_after ─────────────────────────────────────────────────────

    #[test]
    fn paren_depth_after_balanced_is_zero() {
        let tokens = [sym("("), word("x"), sym(")")];
        assert_eq!(paren_depth_after(&tokens), 0);
    }

    #[test]
    fn paren_depth_after_open_paren_returns_one() {
        let tokens = [sym("("), word("x")];
        assert_eq!(paren_depth_after(&tokens), 1);
    }

    #[test]
    fn paren_depth_after_detects_unbalanced_open_via_tokenizer() {
        // Ensure the tokenizer + paren_depth_after pipeline works end-to-end
        let tokens = tokenize_sql("SELECT (a + b FROM dual");
        assert!(
            paren_depth_after(&tokens) > 0,
            "unbalanced open paren should yield depth > 0"
        );
    }

    #[test]
    fn paren_depth_after_balanced_via_tokenizer() {
        let tokens = tokenize_sql("SELECT (a + b) FROM dual");
        assert_eq!(paren_depth_after(&tokens), 0);
    }

    // ── split_top_level_symbol_groups ─────────────────────────────────────────

    fn group_words(groups: Vec<Vec<&SqlToken>>) -> Vec<Vec<String>> {
        groups
            .into_iter()
            .map(|g| {
                g.into_iter()
                    .map(|t| match t {
                        SqlToken::Word(w) => w.clone(),
                        SqlToken::Symbol(s) => s.clone(),
                        SqlToken::String(s) => s.clone(),
                        SqlToken::Comment(c) => c.clone(),
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn split_top_level_symbol_groups_splits_by_comma() {
        let tokens = tokenize_sql("a, b, c");
        let groups = split_top_level_symbol_groups(&tokens, ",");
        assert_eq!(groups.len(), 3);
    }

    #[test]
    fn split_top_level_symbol_groups_ignores_nested_comma() {
        // The `,` inside `(b, c)` must not split at top level
        let tokens = tokenize_sql("a, (b, c), d");
        let groups = split_top_level_symbol_groups(&tokens, ",");
        assert_eq!(
            groups.len(),
            3,
            "expected 3 groups, got {:?}",
            group_words(groups)
        );
    }

    #[test]
    fn split_top_level_symbol_groups_empty_input_returns_empty() {
        let groups = split_top_level_symbol_groups(&[], ",");
        assert!(groups.is_empty());
    }

    #[test]
    fn split_top_level_symbol_groups_no_delimiter_returns_one_group() {
        let tokens = tokenize_sql("a b c");
        let groups = split_top_level_symbol_groups(&tokens, ",");
        assert_eq!(groups.len(), 1);
    }

    #[test]
    fn split_top_level_symbol_groups_string_literal_comma_not_split() {
        // A `,` inside a string literal must not cause a split
        let tokens = tokenize_sql("'a,b', c");
        let groups = split_top_level_symbol_groups(&tokens, ",");
        assert_eq!(
            groups.len(),
            2,
            "string literal comma must not split, got {:?}",
            group_words(groups)
        );
    }

    #[test]
    fn split_top_level_symbol_groups_q_quote_comma_not_split() {
        // Oracle q-quote must be tokenized as a string; commas inside must not split.
        let tokens = tokenize_sql("q'[a,b]', c");
        let groups = split_top_level_symbol_groups(&tokens, ",");
        assert_eq!(
            groups.len(),
            2,
            "q-quote comma must not split, got {:?}",
            group_words(groups)
        );
    }

    #[test]
    fn split_top_level_symbol_groups_dollar_quote_comma_not_split() {
        // PostgreSQL dollar-quoted strings are emitted as SqlToken::String.
        let tokens = tokenize_sql("$$a,b$$, c");
        let groups = split_top_level_symbol_groups(&tokens, ",");
        assert_eq!(
            groups.len(),
            2,
            "dollar-quote comma must not split, got {:?}",
            group_words(groups)
        );
    }

    #[test]
    fn split_top_level_symbol_groups_quoted_identifier_parens_do_not_change_depth() {
        // Parentheses inside quoted identifiers are SqlToken::Word and must not change depth.
        let tokens = tokenize_sql("\"A(B)\", c");
        let groups = split_top_level_symbol_groups(&tokens, ",");
        assert_eq!(
            groups.len(),
            2,
            "quoted identifier parens must not block top-level split, got {:?}",
            group_words(groups)
        );
    }

    #[test]
    fn split_top_level_symbol_groups_ignores_nested_comma_in_brackets() {
        let tokens = tokenize_sql("a, [b, c], d");
        let groups = split_top_level_symbol_groups(&tokens, ",");
        assert_eq!(
            groups.len(),
            3,
            "expected bracket depth to block split, got {:?}",
            group_words(groups)
        );
    }

    #[test]
    fn split_top_level_symbol_groups_ignores_nested_comma_in_braces() {
        let tokens = tokenize_sql("a, {b, c}, d");
        let groups = split_top_level_symbol_groups(&tokens, ",");
        assert_eq!(
            groups.len(),
            3,
            "expected brace depth to block split, got {:?}",
            group_words(groups)
        );
    }

    // ── split_top_level_keyword_groups ────────────────────────────────────────

    #[test]
    fn split_top_level_keyword_groups_splits_select_from_where() {
        let tokens = tokenize_sql("SELECT a FROM t WHERE x = 1");
        let groups = split_top_level_keyword_groups(&tokens, &["FROM", "WHERE"]);
        assert_eq!(groups.len(), 3, "expected SELECT/FROM/WHERE groups");
    }

    #[test]
    fn split_top_level_keyword_groups_ignores_nested_keyword() {
        // FROM inside subquery parens must not split the outer token stream
        let tokens = tokenize_sql("SELECT a, (SELECT b FROM inner_t) FROM outer_t");
        let groups = split_top_level_keyword_groups(&tokens, &["FROM"]);
        assert_eq!(
            groups.len(),
            2,
            "inner FROM must not split outer groups, got {:?}",
            group_words(groups)
        );
    }

    #[test]
    fn split_top_level_keyword_groups_case_insensitive() {
        let tokens = tokenize_sql("select a from t");
        let groups = split_top_level_keyword_groups(&tokens, &["FROM"]);
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn split_top_level_keyword_groups_empty_input_returns_empty() {
        let groups = split_top_level_keyword_groups(&[], &["FROM"]);
        assert!(groups.is_empty());
    }

    // ── paren_depth_after: additional edge cases ──────────────────────────────

    #[test]
    fn paren_depth_after_all_unbalanced_close_parens_is_zero() {
        // Excess `)` tokens must saturate at 0 and not underflow.
        let tokens = [sym(")"), sym(")"), sym(")")];
        assert_eq!(paren_depth_after(&tokens), 0);
    }

    #[test]
    fn paren_depth_after_mismatched_closer_preserves_existing_depth() {
        let tokens = [sym("("), sym("["), word("x"), sym(")")];
        assert_eq!(paren_depth_after(&tokens), 2);
    }

    #[test]
    fn paren_depth_after_mixed_group_symbols_inside_one_token() {
        // "([" opens two levels and "])" should close both levels in order.
        let tokens = [sym("(["), word("x"), sym("])")];
        assert_eq!(paren_depth_after(&tokens), 0);
    }

    #[test]
    fn paren_depth_after_unknown_mismatched_closer_preserves_existing_depth() {
        let tokens = [sym("("), sym("["), word("x"), sym("}"), word("y")];
        assert_eq!(paren_depth_after(&tokens), 2);
    }

    #[test]
    fn split_top_level_symbol_groups_mismatched_closer_keeps_nested_group() {
        let tokens = tokenize_sql("([x), y");
        let groups = split_top_level_symbol_groups(&tokens, ",");
        assert_eq!(
            groups.len(),
            1,
            "mismatched closer must keep existing nested depth, got {:?}",
            group_words(groups)
        );
    }

    #[test]
    fn split_top_level_symbol_groups_unknown_mismatched_closer_keeps_nested_group() {
        let tokens = tokenize_sql("([x}), y");
        let groups = split_top_level_symbol_groups(&tokens, ",");
        assert_eq!(
            groups.len(),
            1,
            "unknown mismatched closer must keep existing nested depth, got {:?}",
            group_words(groups)
        );
    }

    #[test]
    fn split_top_level_keyword_groups_mismatched_closer_keeps_nested_group() {
        let tokens = tokenize_sql("SELECT ([x), y FROM dual");
        let groups = split_top_level_keyword_groups(&tokens, &["FROM"]);
        assert_eq!(
            groups.len(),
            1,
            "mismatched closer must keep keyword split blocked by depth, got {:?}",
            group_words(groups)
        );
    }

    // ── split_top_level_symbol_groups: additional edge cases ──────────────────

    #[test]
    fn split_top_level_symbol_groups_skips_leading_delimiter() {
        // A delimiter at the very start produces no empty leading group.
        let tokens = tokenize_sql(",a");
        let groups = split_top_level_symbol_groups(&tokens, ",");
        assert_eq!(
            groups.len(),
            1,
            "leading delimiter must not create an empty prefix group"
        );
    }

    #[test]
    fn split_top_level_symbol_groups_skips_trailing_delimiter() {
        // A delimiter at the very end produces no empty trailing group.
        let tokens = tokenize_sql("a,");
        let groups = split_top_level_symbol_groups(&tokens, ",");
        assert_eq!(
            groups.len(),
            1,
            "trailing delimiter must not create an empty suffix group"
        );
    }

    #[test]
    fn split_top_level_symbol_groups_skips_consecutive_delimiters() {
        // Two consecutive top-level delimiters produce no empty middle group.
        let tokens = tokenize_sql("a,,b");
        let groups = split_top_level_symbol_groups(&tokens, ",");
        assert_eq!(
            groups.len(),
            2,
            "consecutive delimiters must not create empty intermediate segments, got {:?}",
            group_words(groups)
        );
    }

    #[test]
    fn split_top_level_symbol_groups_unbalanced_close_paren_at_root_stays_in_group() {
        // An unmatched `)` at depth 0 does not affect depth (saturates at 0).
        // It is treated as a plain symbol and ends up in the current group.
        let tokens = [
            word("a"),
            sym(","),
            word("b"),
            sym(")"),
            sym(","),
            word("c"),
        ];
        let groups = split_top_level_symbol_groups(&tokens, ",");
        // Expected: [a], [b, )], [c]
        assert_eq!(
            groups.len(),
            3,
            "unmatched ')' at root should not collapse into adjacent groups, got {:?}",
            group_words(groups)
        );
        assert_eq!(
            groups[1].len(),
            2,
            "the unmatched ')' must be included in its own group, got {:?}",
            group_words(groups)
        );
    }

    // ── split_top_level_keyword_groups: additional edge cases ─────────────────

    #[test]
    fn split_top_level_keyword_groups_leading_break_keyword_no_empty_prefix() {
        // When the very first token is a break keyword, no empty group is emitted
        // before it — the keyword simply starts the first group.
        let tokens = tokenize_sql("FROM t");
        let groups = split_top_level_keyword_groups(&tokens, &["FROM"]);
        assert_eq!(
            groups.len(),
            1,
            "no empty group before a leading break keyword"
        );
        assert!(
            matches!(&groups[0][0], SqlToken::Word(w) if w.eq_ignore_ascii_case("FROM")),
            "the break keyword must be the first token of the first group"
        );
    }

    #[test]
    fn split_top_level_keyword_groups_trailing_break_keyword_forms_singleton_group() {
        // When the last token is a break keyword it must become its own group.
        let tokens = tokenize_sql("SELECT a FROM");
        let groups = split_top_level_keyword_groups(&tokens, &["FROM"]);
        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups[1].len(),
            1,
            "trailing break keyword must form a singleton group, got {:?}",
            group_words(groups)
        );
    }

    #[test]
    fn split_top_level_keyword_groups_keyword_is_first_token_of_each_group() {
        // Each break keyword must be preserved as the first token of its group.
        let tokens = tokenize_sql("SELECT a FROM t WHERE x = 1");
        let groups = split_top_level_keyword_groups(&tokens, &["FROM", "WHERE"]);
        assert_eq!(groups.len(), 3);
        assert!(
            matches!(&groups[1][0], SqlToken::Word(w) if w.eq_ignore_ascii_case("FROM")),
            "FROM must be the first token of group[1]"
        );
        assert!(
            matches!(&groups[2][0], SqlToken::Word(w) if w.eq_ignore_ascii_case("WHERE")),
            "WHERE must be the first token of group[2]"
        );
    }

    // ── depth_at / is_top_level_depth / is_depth ──────────────────────────────

    #[test]
    fn depth_at_out_of_range_returns_zero() {
        assert_eq!(depth_at(&[1, 2], 5), 0);
    }

    #[test]
    fn is_top_level_depth_true_when_zero() {
        let tokens = tokenize_sql("SELECT a FROM t");
        let depths = paren_depths(&tokens);
        assert!(is_top_level_depth(&depths, 0));
    }

    #[test]
    fn is_top_level_depth_false_inside_parens() {
        // Tokens: SELECT, (, a, ), FROM, t
        // Depths:       0   0   1  1    0   0
        // index 2 (a) is at depth 1
        let tokens = tokenize_sql("SELECT (a) FROM t");
        let depths = paren_depths(&tokens);
        let paren_open_idx = depths
            .iter()
            .position(|&d| d == 1)
            .expect("depth 1 not found");
        assert!(!is_top_level_depth(&depths, paren_open_idx));
    }
}
