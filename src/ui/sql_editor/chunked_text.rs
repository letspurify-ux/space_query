use std::sync::Arc;

const TEXT_CHUNK_TARGET_BYTES: usize = 32 * 1024;
const VALUE_CHUNK_TARGET_LEN: usize = 4 * 1024;

trait SequenceLeaf: Clone {
    fn len(&self) -> usize;
    fn secondary_len(&self) -> usize;
    fn secondary_count_before(&self, position: usize) -> usize;
    fn secondary_position(&self, ordinal: usize) -> Option<usize>;
    fn split_at(&self, position: usize) -> (Option<Self>, Option<Self>);
    fn rechunk_pair(left: &Self, right: &Self) -> Vec<Self>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SequenceNode<L> {
    Leaf(L),
    Branch {
        left: Arc<SequenceNode<L>>,
        right: Arc<SequenceNode<L>>,
        len: usize,
        secondary_len: usize,
        leaf_count: usize,
        height: usize,
    },
}

impl<L: SequenceLeaf> SequenceNode<L> {
    fn len(&self) -> usize {
        match self {
            Self::Leaf(leaf) => leaf.len(),
            Self::Branch { len, .. } => *len,
        }
    }

    fn secondary_len(&self) -> usize {
        match self {
            Self::Leaf(leaf) => leaf.secondary_len(),
            Self::Branch { secondary_len, .. } => *secondary_len,
        }
    }

    fn leaf_count(&self) -> usize {
        match self {
            Self::Leaf(_) => 1,
            Self::Branch { leaf_count, .. } => *leaf_count,
        }
    }

    fn height(&self) -> usize {
        match self {
            Self::Leaf(_) => 1,
            Self::Branch { height, .. } => *height,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PersistentSequence<L> {
    root: Option<Arc<SequenceNode<L>>>,
}

impl<L> Default for PersistentSequence<L> {
    fn default() -> Self {
        Self { root: None }
    }
}

impl<L: SequenceLeaf> PersistentSequence<L> {
    fn from_leaves(leaves: Vec<L>) -> Self {
        Self {
            root: build_balanced_nodes(&leaves),
        }
    }

    fn len(&self) -> usize {
        self.root.as_deref().map_or(0, SequenceNode::len)
    }

    fn secondary_len(&self) -> usize {
        self.root.as_deref().map_or(0, SequenceNode::secondary_len)
    }

    #[cfg(test)]
    fn leaf_count(&self) -> usize {
        self.root.as_deref().map_or(0, SequenceNode::leaf_count)
    }

    #[cfg(test)]
    fn height(&self) -> usize {
        self.root.as_deref().map_or(0, SequenceNode::height)
    }

    fn locate(&self, position: usize) -> Option<(&L, usize)> {
        if position >= self.len() {
            return None;
        }
        locate_node(self.root.as_deref()?, position)
    }

    fn replace_range(&mut self, start: usize, end: usize, replacement: Self) {
        let start = start.min(self.len());
        let end = end.min(self.len()).max(start);
        let (left, tail) = split_node(self.root.take(), start);
        let (_, right) = split_node(tail, end.saturating_sub(start));
        self.root = concat_normalized(concat_normalized(left, replacement.root), right);
    }

    fn visit_range(&self, start: usize, end: usize, mut visit: impl FnMut(&L, usize, usize)) {
        let start = start.min(self.len());
        let end = end.min(self.len()).max(start);
        if let Some(root) = self.root.as_deref() {
            visit_node_range(root, 0, start, end, &mut visit);
        }
    }

    fn for_each_leaf(&self, mut visit: impl FnMut(&L)) {
        if let Some(root) = self.root.as_deref() {
            visit_each_leaf(root, &mut visit);
        }
    }

    fn secondary_count_before(&self, position: usize) -> usize {
        self.root.as_deref().map_or(0, |root| {
            secondary_count_before_node(root, position.min(root.len()))
        })
    }

    fn nth_secondary(&self, ordinal: usize) -> Option<usize> {
        let root = self.root.as_deref()?;
        if ordinal >= root.secondary_len() {
            return None;
        }
        nth_secondary_in_node(root, ordinal, 0)
    }

    #[cfg(test)]
    fn shares_root_with(&self, other: &Self) -> bool {
        match (&self.root, &other.root) {
            (Some(left), Some(right)) => Arc::ptr_eq(left, right),
            (None, None) => true,
            _ => false,
        }
    }
}

fn build_balanced_nodes<L: SequenceLeaf>(leaves: &[L]) -> Option<Arc<SequenceNode<L>>> {
    match leaves.len() {
        0 => None,
        1 => Some(Arc::new(SequenceNode::Leaf(leaves[0].clone()))),
        _ => {
            let middle = crate::utils::arithmetic::safe_div(leaves.len(), 2);
            let left = build_balanced_nodes(&leaves[..middle])?;
            let right = build_balanced_nodes(&leaves[middle..])?;
            Some(make_branch(left, right))
        }
    }
}

fn make_branch<L: SequenceLeaf>(
    left: Arc<SequenceNode<L>>,
    right: Arc<SequenceNode<L>>,
) -> Arc<SequenceNode<L>> {
    Arc::new(SequenceNode::Branch {
        len: left.len().saturating_add(right.len()),
        secondary_len: left.secondary_len().saturating_add(right.secondary_len()),
        leaf_count: left.leaf_count().saturating_add(right.leaf_count()),
        height: left.height().max(right.height()).saturating_add(1),
        left,
        right,
    })
}

fn balance_nodes<L: SequenceLeaf>(
    left: Arc<SequenceNode<L>>,
    right: Arc<SequenceNode<L>>,
) -> Arc<SequenceNode<L>> {
    if left.height() > right.height().saturating_add(1) {
        let SequenceNode::Branch {
            left: left_left,
            right: left_right,
            ..
        } = left.as_ref()
        else {
            return make_branch(left, right);
        };
        if left_left.height() >= left_right.height() {
            return make_branch(left_left.clone(), make_branch(left_right.clone(), right));
        }
        let SequenceNode::Branch {
            left: middle_left,
            right: middle_right,
            ..
        } = left_right.as_ref()
        else {
            return make_branch(left, right);
        };
        return make_branch(
            make_branch(left_left.clone(), middle_left.clone()),
            make_branch(middle_right.clone(), right),
        );
    }

    if right.height() > left.height().saturating_add(1) {
        let SequenceNode::Branch {
            left: right_left,
            right: right_right,
            ..
        } = right.as_ref()
        else {
            return make_branch(left, right);
        };
        if right_right.height() >= right_left.height() {
            return make_branch(make_branch(left, right_left.clone()), right_right.clone());
        }
        let SequenceNode::Branch {
            left: middle_left,
            right: middle_right,
            ..
        } = right_left.as_ref()
        else {
            return make_branch(left, right);
        };
        return make_branch(
            make_branch(left, middle_left.clone()),
            make_branch(middle_right.clone(), right_right.clone()),
        );
    }

    make_branch(left, right)
}

fn concat_raw<L: SequenceLeaf>(
    left: Option<Arc<SequenceNode<L>>>,
    right: Option<Arc<SequenceNode<L>>>,
) -> Option<Arc<SequenceNode<L>>> {
    let (left, right) = match (left, right) {
        (None, right) => return right,
        (left, None) => return left,
        (Some(left), Some(right)) => (left, right),
    };

    if left.height() > right.height().saturating_add(1) {
        let SequenceNode::Branch {
            left: left_left,
            right: left_right,
            ..
        } = left.as_ref()
        else {
            return Some(make_branch(left, right));
        };
        let joined_right = concat_raw(Some(left_right.clone()), Some(right))?;
        return Some(balance_nodes(left_left.clone(), joined_right));
    }

    if right.height() > left.height().saturating_add(1) {
        let SequenceNode::Branch {
            left: right_left,
            right: right_right,
            ..
        } = right.as_ref()
        else {
            return Some(make_branch(left, right));
        };
        let joined_left = concat_raw(Some(left), Some(right_left.clone()))?;
        return Some(balance_nodes(joined_left, right_right.clone()));
    }

    Some(make_branch(left, right))
}

fn concat_normalized<L: SequenceLeaf>(
    left: Option<Arc<SequenceNode<L>>>,
    right: Option<Arc<SequenceNode<L>>>,
) -> Option<Arc<SequenceNode<L>>> {
    let (left, right) = match (left, right) {
        (None, right) => return right,
        (left, None) => return left,
        (Some(left), Some(right)) => (left, right),
    };
    let (left_rest, left_leaf) = pop_last(left);
    let (right_rest, right_leaf) = pop_first(right);
    let middle = build_balanced_nodes(&L::rechunk_pair(&left_leaf, &right_leaf));
    concat_raw(concat_raw(left_rest, middle), right_rest)
}

fn split_node<L: SequenceLeaf>(
    node: Option<Arc<SequenceNode<L>>>,
    position: usize,
) -> (Option<Arc<SequenceNode<L>>>, Option<Arc<SequenceNode<L>>>) {
    let Some(node) = node else {
        return (None, None);
    };
    if position == 0 {
        return (None, Some(node));
    }
    if position >= node.len() {
        return (Some(node), None);
    }

    match node.as_ref() {
        SequenceNode::Leaf(leaf) => {
            let (left, right) = leaf.split_at(position);
            (
                left.map(|leaf| Arc::new(SequenceNode::Leaf(leaf))),
                right.map(|leaf| Arc::new(SequenceNode::Leaf(leaf))),
            )
        }
        SequenceNode::Branch { left, right, .. } => {
            let left_len = left.len();
            if position == left_len {
                return (Some(left.clone()), Some(right.clone()));
            }
            if position < left_len {
                let (before, after) = split_node(Some(left.clone()), position);
                (before, concat_raw(after, Some(right.clone())))
            } else {
                let (before, after) =
                    split_node(Some(right.clone()), position.saturating_sub(left_len));
                (concat_raw(Some(left.clone()), before), after)
            }
        }
    }
}

fn pop_first<L: SequenceLeaf>(node: Arc<SequenceNode<L>>) -> (Option<Arc<SequenceNode<L>>>, L) {
    match node.as_ref() {
        SequenceNode::Leaf(leaf) => (None, leaf.clone()),
        SequenceNode::Branch { left, right, .. } => {
            let (left_rest, first) = pop_first(left.clone());
            (concat_raw(left_rest, Some(right.clone())), first)
        }
    }
}

fn pop_last<L: SequenceLeaf>(node: Arc<SequenceNode<L>>) -> (Option<Arc<SequenceNode<L>>>, L) {
    match node.as_ref() {
        SequenceNode::Leaf(leaf) => (None, leaf.clone()),
        SequenceNode::Branch { left, right, .. } => {
            let (right_rest, last) = pop_last(right.clone());
            (concat_raw(Some(left.clone()), right_rest), last)
        }
    }
}

fn locate_node<L: SequenceLeaf>(node: &SequenceNode<L>, position: usize) -> Option<(&L, usize)> {
    match node {
        SequenceNode::Leaf(leaf) => Some((leaf, position.min(leaf.len()))),
        SequenceNode::Branch { left, right, .. } => {
            if position < left.len() {
                locate_node(left, position)
            } else {
                locate_node(right, position.saturating_sub(left.len()))
            }
        }
    }
}

fn visit_node_range<L: SequenceLeaf>(
    node: &SequenceNode<L>,
    node_start: usize,
    start: usize,
    end: usize,
    visit: &mut impl FnMut(&L, usize, usize),
) {
    let node_end = node_start.saturating_add(node.len());
    if start >= node_end || end <= node_start {
        return;
    }
    match node {
        SequenceNode::Leaf(leaf) => {
            let local_start = start.saturating_sub(node_start).min(leaf.len());
            let local_end = end.saturating_sub(node_start).min(leaf.len());
            if local_start < local_end {
                visit(leaf, local_start, local_end);
            }
        }
        SequenceNode::Branch { left, right, .. } => {
            visit_node_range(left, node_start, start, end, visit);
            visit_node_range(
                right,
                node_start.saturating_add(left.len()),
                start,
                end,
                visit,
            );
        }
    }
}

fn visit_each_leaf<L: SequenceLeaf>(node: &SequenceNode<L>, visit: &mut impl FnMut(&L)) {
    match node {
        SequenceNode::Leaf(leaf) => visit(leaf),
        SequenceNode::Branch { left, right, .. } => {
            visit_each_leaf(left, visit);
            visit_each_leaf(right, visit);
        }
    }
}

fn secondary_count_before_node<L: SequenceLeaf>(node: &SequenceNode<L>, position: usize) -> usize {
    match node {
        SequenceNode::Leaf(leaf) => leaf.secondary_count_before(position.min(leaf.len())),
        SequenceNode::Branch { left, right, .. } => {
            if position <= left.len() {
                secondary_count_before_node(left, position)
            } else {
                left.secondary_len()
                    .saturating_add(secondary_count_before_node(
                        right,
                        position.saturating_sub(left.len()),
                    ))
            }
        }
    }
}

fn nth_secondary_in_node<L: SequenceLeaf>(
    node: &SequenceNode<L>,
    ordinal: usize,
    node_start: usize,
) -> Option<usize> {
    match node {
        SequenceNode::Leaf(leaf) => leaf
            .secondary_position(ordinal)
            .map(|position| node_start.saturating_add(position)),
        SequenceNode::Branch { left, right, .. } => {
            if ordinal < left.secondary_len() {
                nth_secondary_in_node(left, ordinal, node_start)
            } else {
                nth_secondary_in_node(
                    right,
                    ordinal.saturating_sub(left.secondary_len()),
                    node_start.saturating_add(left.len()),
                )
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TextChunk {
    text: Arc<str>,
    line_breaks: Arc<[usize]>,
}

impl TextChunk {
    fn new(text: String) -> Self {
        let line_breaks = line_break_positions(text.as_bytes()).into();
        Self {
            text: Arc::from(text),
            line_breaks,
        }
    }
}

impl SequenceLeaf for TextChunk {
    fn len(&self) -> usize {
        self.text.len()
    }

    fn secondary_len(&self) -> usize {
        self.line_breaks.len()
    }

    fn secondary_count_before(&self, position: usize) -> usize {
        self.line_breaks
            .partition_point(|line_break| *line_break < position)
    }

    fn secondary_position(&self, ordinal: usize) -> Option<usize> {
        self.line_breaks.get(ordinal).copied()
    }

    fn split_at(&self, position: usize) -> (Option<Self>, Option<Self>) {
        let position = position.min(self.len());
        let left = self
            .text
            .get(..position)
            .filter(|text| !text.is_empty())
            .map(|text| Self::new(text.to_string()));
        let right = self
            .text
            .get(position..)
            .filter(|text| !text.is_empty())
            .map(|text| Self::new(text.to_string()));
        (left, right)
    }

    fn rechunk_pair(left: &Self, right: &Self) -> Vec<Self> {
        let mut combined = String::with_capacity(left.len().saturating_add(right.len()));
        combined.push_str(&left.text);
        combined.push_str(&right.text);
        split_text_chunks(&combined)
    }
}

/// Persistent UTF-8 rope used by editor-side mirrors and undo snapshots.
///
/// Every internal node caches byte and line-break counts. Clones share the
/// complete root; an edit copies only the balanced-tree paths and at most two
/// boundary chunks, so document-size metadata is never rebuilt linearly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ChunkedText {
    tree: PersistentSequence<TextChunk>,
}

impl ChunkedText {
    pub(crate) fn from_string(text: String) -> Self {
        Self::from_str(&text)
    }

    pub(crate) fn from_str(text: &str) -> Self {
        Self {
            tree: PersistentSequence::from_leaves(split_text_chunks(text)),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.tree.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[cfg(test)]
    pub(crate) fn chunk_count(&self) -> usize {
        self.tree.leaf_count()
    }

    #[cfg(test)]
    pub(crate) fn tree_height(&self) -> usize {
        self.tree.height()
    }

    #[cfg(test)]
    pub(crate) fn shared_chunk_count(&self, other: &Self) -> usize {
        let mut left_chunks = Vec::new();
        let mut right_chunks = Vec::new();
        self.tree
            .for_each_leaf(|chunk| left_chunks.push(chunk.text.clone()));
        other
            .tree
            .for_each_leaf(|chunk| right_chunks.push(chunk.text.clone()));
        left_chunks
            .iter()
            .filter(|left| right_chunks.iter().any(|right| Arc::ptr_eq(left, right)))
            .count()
    }

    #[cfg(test)]
    pub(crate) fn shares_storage_with(&self, other: &Self) -> bool {
        self.tree.shares_root_with(&other.tree)
    }

    #[cfg(test)]
    pub(crate) fn starts_with(&self, prefix: impl ToString) -> bool {
        let prefix = prefix.to_string();
        self.range_string(0, prefix.len())
            .is_some_and(|head| head == prefix)
    }

    #[cfg(test)]
    pub(crate) fn to_flat_string(&self) -> String {
        self.range_string(0, self.len()).unwrap_or_default()
    }

    pub(crate) fn matches_str(&self, other: &str) -> bool {
        if self.len() != other.len() {
            return false;
        }
        let mut offset = 0usize;
        let mut matches = true;
        self.tree.for_each_leaf(|chunk| {
            if matches {
                let end = offset.saturating_add(chunk.len());
                matches = other.get(offset..end) == Some(chunk.text.as_ref());
                offset = end;
            }
        });
        matches && offset == other.len()
    }

    pub(crate) fn byte_at(&self, position: usize) -> Option<u8> {
        let (chunk, offset) = self.tree.locate(position)?;
        chunk.text.as_bytes().get(offset).copied()
    }

    pub(crate) fn is_char_boundary(&self, position: usize) -> bool {
        if position == 0 || position == self.len() {
            return true;
        }
        self.byte_at(position)
            .is_some_and(|byte| byte & 0b1100_0000 != 0b1000_0000)
    }

    pub(crate) fn clamp_boundary(&self, position: usize) -> usize {
        let mut position = position.min(self.len());
        while position > 0 && !self.is_char_boundary(position) {
            position -= 1;
        }
        position
    }

    pub(crate) fn range_string(&self, start: usize, end: usize) -> Option<String> {
        let start = self.clamp_boundary(start.min(self.len()));
        let end = self.clamp_boundary(end.min(self.len()));
        if end < start {
            return Some(String::new());
        }
        let mut result = String::with_capacity(end.saturating_sub(start));
        let mut valid = true;
        self.tree
            .visit_range(start, end, |chunk, local_start, local_end| {
                if let Some(text) = chunk.text.get(local_start..local_end) {
                    result.push_str(text);
                } else {
                    valid = false;
                }
            });
        (valid && result.len() == end.saturating_sub(start)).then_some(result)
    }

    pub(crate) fn replace_range(&mut self, start: usize, end: usize, inserted: &str) -> bool {
        let start = self.clamp_boundary(start.min(self.len()));
        let end = self.clamp_boundary(end.min(self.len()));
        if end < start {
            return false;
        }
        self.tree.replace_range(
            start,
            end,
            PersistentSequence::from_leaves(split_text_chunks(inserted)),
        );
        true
    }

    pub(crate) fn line_count(&self) -> usize {
        if self.is_empty() {
            0
        } else {
            self.tree.secondary_len().saturating_add(1)
        }
    }

    pub(crate) fn line_index_for_position(&self, position: usize) -> usize {
        self.tree.secondary_count_before(position.min(self.len()))
    }

    pub(crate) fn line_start(&self, position: usize) -> usize {
        self.line_start_for_index(self.line_index_for_position(position))
    }

    pub(crate) fn line_end(&self, position: usize) -> usize {
        self.nth_line_break(self.line_index_for_position(position))
            .unwrap_or(self.len())
    }

    pub(crate) fn line_start_for_index(&self, line_index: usize) -> usize {
        if line_index == 0 {
            return 0;
        }
        self.nth_line_break(line_index.saturating_sub(1))
            .map(|position| position.saturating_add(1))
            .unwrap_or(self.len())
    }

    pub(crate) fn inclusive_line_end_for_index(&self, line_index: usize) -> usize {
        self.nth_line_break(line_index)
            .map(|position| position.saturating_add(1).min(self.len()))
            .unwrap_or(self.len())
    }

    fn nth_line_break(&self, ordinal: usize) -> Option<usize> {
        self.tree.nth_secondary(ordinal)
    }
}

impl From<String> for ChunkedText {
    fn from(value: String) -> Self {
        Self::from_string(value)
    }
}

impl From<&str> for ChunkedText {
    fn from(value: &str) -> Self {
        Self::from_str(value)
    }
}

impl PartialEq<&str> for ChunkedText {
    fn eq(&self, other: &&str) -> bool {
        self.matches_str(other)
    }
}

impl PartialEq<String> for ChunkedText {
    fn eq(&self, other: &String) -> bool {
        self.matches_str(other)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ValueChunk<T> {
    values: Arc<[T]>,
}

impl<T: Clone> SequenceLeaf for ValueChunk<T> {
    fn len(&self) -> usize {
        self.values.len()
    }

    fn secondary_len(&self) -> usize {
        0
    }

    fn secondary_count_before(&self, _position: usize) -> usize {
        0
    }

    fn secondary_position(&self, _ordinal: usize) -> Option<usize> {
        None
    }

    fn split_at(&self, position: usize) -> (Option<Self>, Option<Self>) {
        let position = position.min(self.len());
        let left = self
            .values
            .get(..position)
            .filter(|values| !values.is_empty())
            .map(|values| Self {
                values: Arc::from(values.to_vec()),
            });
        let right = self
            .values
            .get(position..)
            .filter(|values| !values.is_empty())
            .map(|values| Self {
                values: Arc::from(values.to_vec()),
            });
        (left, right)
    }

    fn rechunk_pair(left: &Self, right: &Self) -> Vec<Self> {
        let mut combined = Vec::with_capacity(left.len().saturating_add(right.len()));
        combined.extend_from_slice(&left.values);
        combined.extend_from_slice(&right.values);
        value_chunks(&combined)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ChunkedValues<T> {
    tree: PersistentSequence<ValueChunk<T>>,
}

impl<T: Clone> ChunkedValues<T> {
    pub(crate) fn from_vec(values: Vec<T>) -> Self {
        Self {
            tree: PersistentSequence::from_leaves(value_chunks(&values)),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.tree.len()
    }

    pub(crate) fn get(&self, position: usize) -> Option<&T> {
        let (chunk, offset) = self.tree.locate(position)?;
        chunk.values.get(offset)
    }

    pub(crate) fn last(&self) -> Option<&T> {
        self.len()
            .checked_sub(1)
            .and_then(|position| self.get(position))
    }

    pub(crate) fn range_vec(&self, start: usize, end: usize) -> Option<Vec<T>> {
        let start = start.min(self.len());
        let end = end.min(self.len());
        if end < start {
            return None;
        }
        let mut result = Vec::with_capacity(end.saturating_sub(start));
        let mut valid = true;
        self.tree
            .visit_range(start, end, |chunk, local_start, local_end| {
                if let Some(values) = chunk.values.get(local_start..local_end) {
                    result.extend_from_slice(values);
                } else {
                    valid = false;
                }
            });
        (valid && result.len() == end.saturating_sub(start)).then_some(result)
    }

    pub(crate) fn replace_range(&mut self, start: usize, end: usize, replacement: Vec<T>) {
        self.tree.replace_range(
            start,
            end,
            PersistentSequence::from_leaves(value_chunks(&replacement)),
        );
    }

    #[cfg(test)]
    pub(crate) fn to_vec(&self) -> Vec<T> {
        self.range_vec(0, self.len()).unwrap_or_default()
    }

    #[cfg(test)]
    fn tree_height(&self) -> usize {
        self.tree.height()
    }

    #[cfg(test)]
    fn chunk_count(&self) -> usize {
        self.tree.leaf_count()
    }
}

fn value_chunks<T: Clone>(values: &[T]) -> Vec<ValueChunk<T>> {
    values
        .chunks(VALUE_CHUNK_TARGET_LEN)
        .map(|chunk| ValueChunk {
            values: Arc::from(chunk.to_vec()),
        })
        .collect()
}

fn split_text_chunks(text: &str) -> Vec<TextChunk> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < text.len() {
        let mut end = start
            .saturating_add(TEXT_CHUNK_TARGET_BYTES)
            .min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            end = text[start..]
                .char_indices()
                .nth(1)
                .map(|(idx, _)| start + idx)
                .unwrap_or(text.len());
        }
        if end < text.len()
            && text.as_bytes().get(end.saturating_sub(1)) == Some(&b'\r')
            && text.as_bytes().get(end) == Some(&b'\n')
        {
            end += 1;
        }
        chunks.push(TextChunk::new(text[start..end].to_string()));
        start = end;
    }
    chunks
}

fn line_break_positions(bytes: &[u8]) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut idx = 0usize;
    while idx < bytes.len() {
        match bytes[idx] {
            b'\n' => positions.push(idx),
            b'\r' if bytes.get(idx + 1) == Some(&b'\n') => {
                idx += 1;
                positions.push(idx);
            }
            b'\r' => positions.push(idx),
            _ => {}
        }
        idx += 1;
    }
    positions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunked_text_edit_preserves_utf8_and_does_not_copy_document_tail() {
        let source = format!("{}끝", "가나다라\n".repeat(20_000));
        let mut text = ChunkedText::from_str(&source);
        let before = text.clone();
        let edit_at = source.find('\n').unwrap_or(0) + 1;
        assert!(text.replace_range(edit_at, edit_at, "SELECT 1;\n"));
        let flattened = text.to_flat_string();
        assert_eq!(
            flattened,
            format!("{}SELECT 1;\n{}", &source[..edit_at], &source[edit_at..])
        );
        assert!(
            text.shared_chunk_count(&before) > 0,
            "unaffected rope leaves should remain shared"
        );
    }

    #[test]
    fn chunked_text_snapshot_clone_shares_persistent_tree_root() {
        let text = ChunkedText::from_str(&"SELECT 1;\n".repeat(100_000));
        let mut snapshot = text.clone();

        assert!(text.shares_storage_with(&snapshot));
        let edit_at = snapshot.len() / 2;
        assert!(snapshot.replace_range(edit_at, edit_at, "-- pasted\n"));
        assert!(!text.shares_storage_with(&snapshot));
        assert!(snapshot.shared_chunk_count(&text) > 0);
        assert_eq!(text.len() + "-- pasted\n".len(), snapshot.len());
    }

    #[test]
    fn chunked_text_line_queries_match_flat_text() {
        let source = "a\r\nb\nc\rd".repeat(10_000);
        let text = ChunkedText::from_str(&source);
        let breaks = line_break_positions(source.as_bytes());
        for pos in [0, 1, 2, source.len() / 2, source.len()] {
            let expected_start = breaks
                .iter()
                .copied()
                .take_while(|line_break| *line_break < pos)
                .last()
                .map_or(0, |idx| idx + 1);
            assert!(text.line_start(pos) <= pos);
            assert_eq!(text.line_start(pos), expected_start);
        }
        assert_eq!(text.to_flat_string(), source);
    }

    #[test]
    fn persistent_text_tree_height_stays_logarithmic_after_middle_edits() {
        let source = "SELECT 1;\n".repeat(1_000_000);
        let mut text = ChunkedText::from_str(&source);
        for index in 0..256 {
            let middle = text.len() / 2;
            let inserted = if index % 2 == 0 { "x" } else { "yz" };
            assert!(text.replace_range(middle, middle, inserted));
        }
        let leaves = text.chunk_count().max(1);
        let logarithmic_limit = usize::BITS as usize - leaves.leading_zeros() as usize;
        assert!(text.tree_height() <= logarithmic_limit.saturating_mul(2).max(2));
    }

    #[test]
    fn chunked_values_replaces_only_boundary_paths() {
        let mut values = ChunkedValues::from_vec((0..20_000).collect::<Vec<_>>());
        values.replace_range(5_000, 5_003, vec![9, 8]);
        let flat = values.to_vec();
        assert_eq!(&flat[4_998..5_004], &[4_998, 4_999, 9, 8, 5_003, 5_004]);
        assert_eq!(flat.len(), 19_999);
        assert_eq!(
            values.range_vec(4_998, 5_004).as_deref(),
            Some(&flat[4_998..5_004])
        );
    }

    #[test]
    fn chunked_values_tree_keeps_deep_middle_queries_and_edits_aligned() {
        let mut values = ChunkedValues::from_vec((0..1_000_000).collect::<Vec<_>>());
        assert_eq!(values.get(750_000), Some(&750_000));
        assert_eq!(
            values.range_vec(749_998, 750_003),
            Some(vec![749_998, 749_999, 750_000, 750_001, 750_002])
        );

        values.replace_range(750_000, 750_003, vec![7, 8]);

        assert_eq!(values.get(750_000), Some(&7));
        assert_eq!(values.get(750_001), Some(&8));
        assert_eq!(values.get(750_002), Some(&750_003));
        let leaves = values.chunk_count().max(1);
        let logarithmic_limit = usize::BITS as usize - leaves.leading_zeros() as usize;
        assert!(values.tree_height() <= logarithmic_limit.saturating_mul(2).max(2));
    }

    #[test]
    fn edit_at_chunk_boundary_keeps_cross_boundary_crlf_as_one_line_break() {
        let mut source = "x".repeat(TEXT_CHUNK_TARGET_BYTES.saturating_sub(1));
        source.push('\r');
        source.push('z');
        let mut text = ChunkedText::from_str(&source);
        assert!(text.replace_range(TEXT_CHUNK_TARGET_BYTES, TEXT_CHUNK_TARGET_BYTES, "\n"));
        assert_eq!(
            text.line_count(),
            2,
            "a CRLF split by the edit point is one logical line break"
        );
        assert_eq!(
            text.line_start(TEXT_CHUNK_TARGET_BYTES.saturating_add(1)),
            TEXT_CHUNK_TARGET_BYTES.saturating_add(1)
        );
    }
}
