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
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DelimiterFrameState {
    stack: Vec<DelimiterFrameKind>,
}

impl DelimiterFrameState {
    #[inline]
    pub(crate) fn depth(&self) -> usize {
        self.stack.len()
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
                if self.stack.len() < baseline_depth {
                    return true;
                }
            }
        }

        false
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
}
