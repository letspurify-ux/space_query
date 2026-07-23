use crate::ui::sql_editor::SqlToken;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DelimiterFrameKind {
    Unknown,
    Paren,
    Bracket,
    Brace,
}

impl DelimiterFrameKind {
    fn from_open_char(symbol: char) -> Option<Self> {
        match symbol {
            '(' => Some(Self::Paren),
            '[' => Some(Self::Bracket),
            '{' => Some(Self::Brace),
            _ => None,
        }
    }

    fn from_close_char(symbol: char) -> Option<Self> {
        match symbol {
            ')' => Some(Self::Paren),
            ']' => Some(Self::Bracket),
            '}' => Some(Self::Brace),
            _ => None,
        }
    }

    fn can_be_closed_by(self, close_kind: Self) -> bool {
        matches!(self, Self::Unknown) || self == close_kind
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DelimiterLineStartSnapshot {
    visible_frames: Vec<DelimiterFrameKind>,
    baseline_depth: usize,
}

impl DelimiterLineStartSnapshot {
    pub(crate) fn baseline_depth(&self) -> usize {
        self.baseline_depth
    }

    pub(crate) fn frame_state(&self) -> DelimiterFrameState {
        DelimiterFrameState {
            stack: self.visible_frames.clone(),
            close_generations_by_resulting_depth: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DelimiterBoundary {
    depth: usize,
    close_generation_below: u64,
}

impl DelimiterBoundary {
    #[inline]
    pub(crate) fn depth(self) -> usize {
        self.depth
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DelimiterFrameState {
    stack: Vec<DelimiterFrameKind>,
    close_generations_by_resulting_depth: Option<Vec<u64>>,
}

impl DelimiterFrameState {
    pub(crate) fn with_boundary_tracking() -> Self {
        Self {
            stack: Vec::new(),
            close_generations_by_resulting_depth: Some(Vec::new()),
        }
    }

    #[inline]
    pub(crate) fn depth(&self) -> usize {
        self.stack.len()
    }

    pub(crate) fn is_inside_bracket(&self) -> bool {
        self.stack.contains(&DelimiterFrameKind::Bracket)
    }

    pub(crate) fn innermost_is_bracket(&self) -> bool {
        self.stack.last() == Some(&DelimiterFrameKind::Bracket)
    }

    pub(crate) fn boundary(&self) -> DelimiterBoundary {
        let depth = self.depth();
        let close_generation_below = depth
            .checked_sub(1)
            .and_then(|resulting_depth| {
                self.close_generations_by_resulting_depth
                    .as_ref()
                    .and_then(|generations| generations.get(resulting_depth))
            })
            .copied()
            .unwrap_or(0);

        DelimiterBoundary {
            depth,
            close_generation_below,
        }
    }

    pub(crate) fn has_closed_below(&self, boundary: DelimiterBoundary) -> bool {
        let Some(resulting_depth) = boundary.depth.checked_sub(1) else {
            return false;
        };
        self.close_generations_by_resulting_depth
            .as_ref()
            .and_then(|generations| generations.get(resulting_depth))
            .copied()
            .unwrap_or(0)
            != boundary.close_generation_below
    }

    pub(crate) fn apply_token(&mut self, token: &SqlToken) {
        let SqlToken::Symbol(symbol) = token else {
            return;
        };

        self.apply_symbol_strict(symbol);
    }

    pub(crate) fn apply_token_with_close_detection(
        &mut self,
        token: &SqlToken,
        baseline_depth: usize,
    ) -> bool {
        let SqlToken::Symbol(symbol) = token else {
            return false;
        };

        self.apply_symbol_with_close_detection(symbol, baseline_depth)
    }

    pub(crate) fn line_start_snapshot(&self, baseline_depth: usize) -> DelimiterLineStartSnapshot {
        let synthetic_missing_depth = baseline_depth.saturating_sub(self.stack.len());
        let mut visible_frames = vec![DelimiterFrameKind::Unknown; synthetic_missing_depth];
        visible_frames.extend(self.stack.iter().copied());

        DelimiterLineStartSnapshot {
            visible_frames,
            baseline_depth,
        }
    }

    fn apply_symbol_strict(&mut self, symbol: &str) {
        for sym_ch in symbol.chars() {
            if let Some(open_kind) = DelimiterFrameKind::from_open_char(sym_ch) {
                self.stack.push(open_kind);
                continue;
            }

            let Some(close_kind) = DelimiterFrameKind::from_close_char(sym_ch) else {
                continue;
            };

            if self
                .stack
                .last()
                .copied()
                .is_some_and(|top| top == close_kind)
            {
                let _ = self.stack.pop();
                self.record_successful_close();
            }
        }
    }

    fn apply_symbol_with_close_detection(&mut self, symbol: &str, baseline_depth: usize) -> bool {
        for sym_ch in symbol.chars() {
            if let Some(open_kind) = DelimiterFrameKind::from_open_char(sym_ch) {
                self.stack.push(open_kind);
                continue;
            }

            let Some(close_kind) = DelimiterFrameKind::from_close_char(sym_ch) else {
                continue;
            };

            if self
                .stack
                .last()
                .copied()
                .is_some_and(|top| top.can_be_closed_by(close_kind))
            {
                let _ = self.stack.pop();
                self.record_successful_close();
                if self.stack.len() < baseline_depth {
                    return true;
                }
            }
        }

        false
    }

    fn record_successful_close(&mut self) {
        let Some(close_generations_by_resulting_depth) =
            self.close_generations_by_resulting_depth.as_mut()
        else {
            return;
        };
        let resulting_depth = self.stack.len();
        if close_generations_by_resulting_depth.len() <= resulting_depth {
            close_generations_by_resulting_depth.resize(resulting_depth.saturating_add(1), 0);
        }
        if let Some(generation) = close_generations_by_resulting_depth.get_mut(resulting_depth) {
            *generation = generation.wrapping_add(1);
        }
    }
}

pub(crate) fn line_start_snapshot_before_token(
    tokens: &[SqlToken],
    line_start_idx: usize,
    baseline_depth: usize,
) -> DelimiterLineStartSnapshot {
    let mut state = DelimiterFrameState::default();
    for token in tokens.iter().take(line_start_idx) {
        state.apply_token(token);
    }
    state.line_start_snapshot(baseline_depth)
}

pub(crate) fn line_closes_delimiter_frame_below_snapshot_before_token(
    tokens: &[SqlToken],
    line_start_idx: usize,
    token_idx: usize,
    line_start_snapshot: &DelimiterLineStartSnapshot,
) -> bool {
    if line_start_idx >= token_idx || line_start_snapshot.baseline_depth() == 0 {
        return false;
    }

    let mut frame_state = line_start_snapshot.frame_state();
    for token in tokens
        .iter()
        .skip(line_start_idx)
        .take(token_idx.saturating_sub(line_start_idx))
    {
        if frame_state.apply_token_with_close_detection(token, line_start_snapshot.baseline_depth())
        {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::{
        line_closes_delimiter_frame_below_snapshot_before_token, line_start_snapshot_before_token,
        DelimiterFrameState,
    };
    use crate::ui::sql_editor::query_text::tokenize_sql;
    use crate::ui::sql_editor::SqlToken;

    fn comma_index(tokens: &[SqlToken]) -> usize {
        tokens
            .iter()
            .enumerate()
            .find(|(_, token)| matches!(token, SqlToken::Symbol(sym) if sym == ","))
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    }

    #[test]
    fn line_start_snapshot_keeps_visible_stack_when_baseline_is_shallower() {
        let tokens = tokenize_sql("((\n) + value, tail");
        let line_start_idx = tokens
            .iter()
            .enumerate()
            .find(|(_, token)| matches!(token, SqlToken::Symbol(sym) if sym == ")"))
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        let comma_idx = comma_index(&tokens);

        let shallow_snapshot = line_start_snapshot_before_token(&tokens, line_start_idx, 1);
        let deep_snapshot = line_start_snapshot_before_token(&tokens, line_start_idx, 2);

        assert!(!line_closes_delimiter_frame_below_snapshot_before_token(
            &tokens,
            line_start_idx,
            comma_idx,
            &shallow_snapshot,
        ));
        assert!(line_closes_delimiter_frame_below_snapshot_before_token(
            &tokens,
            line_start_idx,
            comma_idx,
            &deep_snapshot,
        ));
    }

    #[test]
    fn line_start_snapshot_inserts_unknown_frames_for_missing_outer_depth() {
        let tokens = tokenize_sql(") + (, tail");
        let comma_idx = comma_index(&tokens);
        let snapshot = line_start_snapshot_before_token(&tokens, 0, 1);

        assert!(line_closes_delimiter_frame_below_snapshot_before_token(
            &tokens, 0, comma_idx, &snapshot,
        ));
    }

    #[test]
    fn boundary_detects_close_even_when_the_same_depth_reopens() {
        let mut state = DelimiterFrameState::with_boundary_tracking();
        state.apply_token(&SqlToken::Symbol("(".to_string()));
        let boundary = state.boundary();

        state.apply_token(&SqlToken::Symbol(")".to_string()));
        state.apply_token(&SqlToken::Symbol("(".to_string()));

        assert_eq!(state.depth(), boundary.depth());
        assert!(state.has_closed_below(boundary));
    }

    #[test]
    fn boundary_ignores_inner_and_mismatched_closes() {
        let mut state = DelimiterFrameState::with_boundary_tracking();
        state.apply_token(&SqlToken::Symbol("(".to_string()));
        let boundary = state.boundary();

        state.apply_token(&SqlToken::Symbol("[".to_string()));
        state.apply_token(&SqlToken::Symbol(")".to_string()));
        assert!(!state.has_closed_below(boundary));

        state.apply_token(&SqlToken::Symbol("]".to_string()));
        assert!(!state.has_closed_below(boundary));
        assert_eq!(state.depth(), boundary.depth());
    }
}
