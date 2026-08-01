pub(crate) type SqlFormatFrameId = u64;

fn frame_id_matches(expected: Option<SqlFormatFrameId>, current: Option<SqlFormatFrameId>) -> bool {
    expected.is_none_or(|id| current == Some(id))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SqlFormatFrameContext {
    query_frame_id: Option<SqlFormatFrameId>,
    owner_relative_frame_id: Option<SqlFormatFrameId>,
}

impl SqlFormatFrameContext {
    pub(crate) fn new(
        query_frame_id: Option<SqlFormatFrameId>,
        owner_relative_frame_id: Option<SqlFormatFrameId>,
    ) -> Self {
        Self {
            query_frame_id,
            owner_relative_frame_id,
        }
    }

    pub(crate) fn matches(self, current: Self) -> bool {
        frame_id_matches(self.query_frame_id, current.query_frame_id)
            && frame_id_matches(
                self.owner_relative_frame_id,
                current.owner_relative_frame_id,
            )
    }
}

pub(crate) fn clear_mismatched_frame_context<T>(
    slot: &mut Option<T>,
    current: SqlFormatFrameContext,
    mut frame_context: impl FnMut(&T) -> SqlFormatFrameContext,
) {
    if slot
        .as_ref()
        .is_some_and(|value| !frame_context(value).matches(current))
    {
        *slot = None;
    }
}

pub(crate) fn filter_matching_frame_context<T>(
    value: Option<T>,
    current: SqlFormatFrameContext,
    mut frame_context: impl FnMut(&T) -> SqlFormatFrameContext,
) -> Option<T> {
    value.filter(|value| frame_context(value).matches(current))
}

pub(crate) fn prune_mismatched_frame_context_tail<T>(
    values: &mut Vec<T>,
    current: SqlFormatFrameContext,
    mut frame_context: impl FnMut(&T) -> SqlFormatFrameContext,
) {
    while values
        .last()
        .is_some_and(|value| !frame_context(value).matches(current))
    {
        let _ = values.pop();
    }
}

pub(crate) fn sync_option_frame_context<T>(
    slot: &mut Option<T>,
    next: SqlFormatFrameContext,
    mut set_frame_context: impl FnMut(&mut T, SqlFormatFrameContext),
) {
    if let Some(value) = slot.as_mut() {
        set_frame_context(value, next);
    }
}

pub(crate) fn sync_slice_frame_context<T>(
    values: &mut [T],
    next: SqlFormatFrameContext,
    mut set_frame_context: impl FnMut(&mut T, SqlFormatFrameContext),
) {
    values
        .iter_mut()
        .for_each(|value| set_frame_context(value, next));
}

pub(crate) fn take_matching_frame_context<T>(
    slot: &mut Option<T>,
    current: SqlFormatFrameContext,
    mut frame_context: impl FnMut(&T) -> SqlFormatFrameContext,
) -> Option<T> {
    if slot
        .as_ref()
        .is_some_and(|value| frame_context(value).matches(current))
    {
        slot.take()
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SqlFormatScopedFrame {
    depth: usize,
    frame_id: Option<SqlFormatFrameId>,
}

impl SqlFormatScopedFrame {
    pub(crate) fn new(depth: usize, frame_id: Option<SqlFormatFrameId>) -> Self {
        Self { depth, frame_id }
    }

    pub(crate) fn contains(self, other: Self) -> bool {
        self.depth <= other.depth && (self.depth < other.depth || self.frame_id == other.frame_id)
    }
}

#[derive(Default)]
pub(crate) struct SqlFormatFrameIdAllocator {
    next_frame_id: SqlFormatFrameId,
}

impl SqlFormatFrameIdAllocator {
    pub(crate) fn next_id(&mut self) -> SqlFormatFrameId {
        let next = self.next_frame_id;
        self.next_frame_id = self.next_frame_id.saturating_add(1);
        next
    }
}

#[cfg(test)]
mod tests {
    use super::{
        clear_mismatched_frame_context, filter_matching_frame_context,
        prune_mismatched_frame_context_tail, sync_option_frame_context,
        take_matching_frame_context, SqlFormatFrameContext, SqlFormatScopedFrame,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct TestFrameCarrier {
        frame_context: SqlFormatFrameContext,
        value: usize,
    }

    #[test]
    fn sql_format_frame_context_matches_only_same_frame_ids() {
        let expected = SqlFormatFrameContext::new(Some(10), Some(20));

        assert!(expected.matches(SqlFormatFrameContext::new(Some(10), Some(20))));
        assert!(!expected.matches(SqlFormatFrameContext::new(Some(11), Some(20))));
        assert!(!expected.matches(SqlFormatFrameContext::new(Some(10), Some(21))));
    }

    #[test]
    fn sql_format_scoped_frame_same_depth_different_ids_do_not_contain_each_other() {
        let left = SqlFormatScopedFrame::new(1, Some(10));
        let right = SqlFormatScopedFrame::new(1, Some(11));

        assert!(!left.contains(right));
        assert!(!right.contains(left));
    }

    #[test]
    fn sql_format_clear_mismatched_frame_context_drops_value() {
        let current = SqlFormatFrameContext::new(Some(1), Some(2));
        let mut value = Some(TestFrameCarrier {
            frame_context: SqlFormatFrameContext::new(Some(1), Some(3)),
            value: 7,
        });

        clear_mismatched_frame_context(&mut value, current, |value| value.frame_context);

        assert_eq!(value, None);
    }

    #[test]
    fn sql_format_take_matching_frame_context_returns_value() {
        let current = SqlFormatFrameContext::new(Some(1), Some(2));
        let mut value = Some(TestFrameCarrier {
            frame_context: current,
            value: 7,
        });

        let taken = take_matching_frame_context(&mut value, current, |value| value.frame_context);

        assert_eq!(
            taken,
            Some(TestFrameCarrier {
                frame_context: current,
                value: 7,
            })
        );
        assert_eq!(value, None);
    }

    #[test]
    fn sql_format_filter_matching_frame_context_keeps_only_matching_value() {
        let current = SqlFormatFrameContext::new(Some(1), Some(2));

        assert_eq!(
            filter_matching_frame_context(
                Some(TestFrameCarrier {
                    frame_context: current,
                    value: 7,
                }),
                current,
                |value| value.frame_context
            ),
            Some(TestFrameCarrier {
                frame_context: current,
                value: 7,
            })
        );
        assert_eq!(
            filter_matching_frame_context(
                Some(TestFrameCarrier {
                    frame_context: SqlFormatFrameContext::new(Some(9), Some(2)),
                    value: 7,
                }),
                current,
                |value| value.frame_context
            ),
            None
        );
    }

    #[test]
    fn sql_format_prune_mismatched_frame_context_tail_removes_only_mismatched_suffix() {
        let current = SqlFormatFrameContext::new(Some(1), None);
        let mut values = vec![
            TestFrameCarrier {
                frame_context: current,
                value: 1,
            },
            TestFrameCarrier {
                frame_context: SqlFormatFrameContext::new(Some(2), None),
                value: 2,
            },
        ];

        prune_mismatched_frame_context_tail(&mut values, current, |value| value.frame_context);

        assert_eq!(
            values,
            vec![TestFrameCarrier {
                frame_context: current,
                value: 1,
            }]
        );
    }

    #[test]
    fn sql_format_sync_option_frame_context_updates_existing_value() {
        let next = SqlFormatFrameContext::new(Some(1), Some(2));
        let mut value = Some(TestFrameCarrier {
            frame_context: SqlFormatFrameContext::default(),
            value: 7,
        });

        sync_option_frame_context(&mut value, next, |value, next| value.frame_context = next);

        assert_eq!(
            value,
            Some(TestFrameCarrier {
                frame_context: next,
                value: 7,
            })
        );
    }
}
