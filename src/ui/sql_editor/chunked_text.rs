use std::sync::Arc;

const TEXT_CHUNK_TARGET_BYTES: usize = 32 * 1024;
const TEXT_CHUNK_RECHUNK_MIN_BYTES: usize = 16 * 1024;
const VALUE_CHUNK_TARGET_LEN: usize = 4 * 1024;
const VALUE_CHUNK_RECHUNK_MIN_LEN: usize = 2 * 1024;

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

#[derive(Clone, Debug)]
struct TextChunk {
    storage: Arc<String>,
    storage_line_breaks: Arc<[usize]>,
    storage_is_ascii: bool,
    start: usize,
    end: usize,
    line_break_start: usize,
    line_break_end: usize,
}

impl TextChunk {
    fn from_storage(
        storage: Arc<String>,
        storage_line_breaks: Arc<[usize]>,
        storage_is_ascii: bool,
        start: usize,
        end: usize,
    ) -> Self {
        let start = start.min(storage.len());
        let end = end.min(storage.len()).max(start);
        let line_break_start = storage_line_breaks.partition_point(|position| *position < start);
        let line_break_end = storage_line_breaks.partition_point(|position| *position < end);
        Self {
            storage,
            storage_line_breaks,
            storage_is_ascii,
            start,
            end,
            line_break_start,
            line_break_end,
        }
    }

    fn as_str(&self) -> &str {
        self.storage.get(self.start..self.end).unwrap_or("")
    }

    #[cfg(test)]
    fn shares_storage_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.storage, &other.storage)
    }

    fn line_breaks(&self) -> &[usize] {
        self.storage_line_breaks
            .get(self.line_break_start..self.line_break_end)
            .unwrap_or_default()
    }
}

impl PartialEq for TextChunk {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for TextChunk {}

impl SequenceLeaf for TextChunk {
    fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    fn secondary_len(&self) -> usize {
        self.line_break_end.saturating_sub(self.line_break_start)
    }

    fn secondary_count_before(&self, position: usize) -> usize {
        let absolute = self.start.saturating_add(position.min(self.len()));
        self.line_breaks()
            .partition_point(|line_break| *line_break < absolute)
    }

    fn secondary_position(&self, ordinal: usize) -> Option<usize> {
        self.line_breaks()
            .get(ordinal)
            .map(|position| position.saturating_sub(self.start))
    }

    fn split_at(&self, position: usize) -> (Option<Self>, Option<Self>) {
        let position = position.min(self.len());
        let absolute = self.start.saturating_add(position).min(self.end);
        let left = (absolute > self.start).then(|| {
            Self::from_storage(
                self.storage.clone(),
                self.storage_line_breaks.clone(),
                self.storage_is_ascii,
                self.start,
                absolute,
            )
        });
        let right = (absolute < self.end).then(|| {
            Self::from_storage(
                self.storage.clone(),
                self.storage_line_breaks.clone(),
                self.storage_is_ascii,
                absolute,
                self.end,
            )
        });
        (left, right)
    }

    fn rechunk_pair(left: &Self, right: &Self) -> Vec<Self> {
        if Arc::ptr_eq(&left.storage, &right.storage)
            && Arc::ptr_eq(&left.storage_line_breaks, &right.storage_line_breaks)
            && left.end == right.start
        {
            return vec![Self::from_storage(
                left.storage.clone(),
                left.storage_line_breaks.clone(),
                left.storage_is_ascii,
                left.start,
                right.end,
            )];
        }

        let combined_len = left.len().saturating_add(right.len());
        let joins_crlf = left.as_str().as_bytes().last() == Some(&b'\r')
            && right.as_str().as_bytes().first() == Some(&b'\n');
        if !joins_crlf
            && combined_len > TEXT_CHUNK_TARGET_BYTES
            && left.len() >= TEXT_CHUNK_RECHUNK_MIN_BYTES
            && right.len() >= TEXT_CHUNK_RECHUNK_MIN_BYTES
        {
            return vec![left.clone(), right.clone()];
        }
        if !joins_crlf
            && combined_len > TEXT_CHUNK_TARGET_BYTES
            && (left.len() >= TEXT_CHUNK_TARGET_BYTES || right.len() >= TEXT_CHUNK_TARGET_BYTES)
        {
            return vec![left.clone(), right.clone()];
        }

        let mut combined = String::with_capacity(left.len().saturating_add(right.len()));
        combined.push_str(left.as_str());
        combined.push_str(right.as_str());
        split_text_chunks_from_string(combined)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ChunkedTextSlice {
    storage: Arc<String>,
    range: std::ops::Range<usize>,
    known_ascii: bool,
}

impl ChunkedTextSlice {
    pub(crate) fn new(storage: Arc<String>, start: usize, end: usize) -> Self {
        let start = start.min(storage.len());
        let end = end.min(storage.len()).max(start);
        Self {
            storage,
            range: start..end,
            known_ascii: false,
        }
    }

    fn new_with_ascii(storage: Arc<String>, start: usize, end: usize, known_ascii: bool) -> Self {
        let mut value = Self::new(storage, start, end);
        value.known_ascii = known_ascii;
        value
    }

    pub(crate) fn whole(storage: Arc<String>) -> Self {
        let end = storage.len();
        let known_ascii = storage.is_ascii();
        Self::new_with_ascii(storage, 0, end, known_ascii)
    }

    pub(crate) fn as_str(&self) -> &str {
        self.storage.get(self.range.clone()).unwrap_or("")
    }

    pub(crate) fn subslice(&self, start: usize, end: usize) -> Self {
        let len = self.range.end.saturating_sub(self.range.start);
        let start = start.min(len);
        let end = end.min(len).max(start);
        Self::new_with_ascii(
            self.storage.clone(),
            self.range.start.saturating_add(start),
            self.range.start.saturating_add(end),
            self.known_ascii,
        )
    }

    pub(crate) fn is_known_ascii(&self) -> bool {
        self.known_ascii
    }

    #[cfg(test)]
    pub(crate) fn shares_storage_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.storage, &other.storage)
    }
}

impl From<String> for ChunkedTextSlice {
    fn from(value: String) -> Self {
        Self::whole(Arc::new(value))
    }
}

impl std::ops::Deref for ChunkedTextSlice {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
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
        Self {
            tree: PersistentSequence::from_leaves(split_text_chunks_from_string(text)),
        }
    }

    pub(crate) fn from_shared_string(text: Arc<String>) -> Self {
        Self {
            tree: PersistentSequence::from_leaves(split_text_chunks_from_storage(text)),
        }
    }

    pub(crate) fn from_str(text: &str) -> Self {
        Self::from_string(text.to_string())
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
            .for_each_leaf(|chunk| left_chunks.push(chunk.clone()));
        other
            .tree
            .for_each_leaf(|chunk| right_chunks.push(chunk.clone()));
        left_chunks
            .iter()
            .filter(|left| {
                right_chunks.iter().any(|right| {
                    left.start == right.start
                        && left.end == right.end
                        && left.shares_storage_with(right)
                })
            })
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
                matches = other.get(offset..end) == Some(chunk.as_str());
                offset = end;
            }
        });
        matches && offset == other.len()
    }

    pub(crate) fn byte_at(&self, position: usize) -> Option<u8> {
        let (chunk, offset) = self.tree.locate(position)?;
        chunk.as_str().as_bytes().get(offset).copied()
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
                if let Some(text) = chunk.as_str().get(local_start..local_end) {
                    result.push_str(text);
                } else {
                    valid = false;
                }
            });
        (valid && result.len() == end.saturating_sub(start)).then_some(result)
    }

    pub(crate) fn shared_range(&self, start: usize, end: usize) -> Option<ChunkedTextSlice> {
        let start = self.clamp_boundary(start.min(self.len()));
        let end = self.clamp_boundary(end.min(self.len()));
        if end < start {
            return None;
        }
        if start == end {
            return Some(ChunkedTextSlice::whole(Arc::new(String::new())));
        }

        let mut storage = None::<Arc<String>>;
        let mut storage_start = 0usize;
        let mut expected_storage_end = 0usize;
        let mut known_ascii = false;
        let mut valid = true;
        self.tree
            .visit_range(start, end, |chunk, local_start, local_end| {
                if !valid {
                    return;
                }
                let absolute_start = chunk.start.saturating_add(local_start);
                let absolute_end = chunk.start.saturating_add(local_end).min(chunk.end);
                match storage.as_ref() {
                    None => {
                        storage = Some(chunk.storage.clone());
                        storage_start = absolute_start;
                        expected_storage_end = absolute_end;
                        known_ascii = chunk.storage_is_ascii;
                    }
                    Some(current)
                        if Arc::ptr_eq(current, &chunk.storage)
                            && absolute_start == expected_storage_end =>
                    {
                        expected_storage_end = absolute_end;
                    }
                    Some(_) => valid = false,
                }
            });
        let storage = storage.filter(|_| valid)?;
        Some(ChunkedTextSlice::new_with_ascii(
            storage,
            storage_start,
            expected_storage_end,
            known_ascii,
        ))
    }

    pub(crate) fn shared_or_owned_range(
        &self,
        start: usize,
        end: usize,
    ) -> Option<ChunkedTextSlice> {
        self.shared_range(start, end).or_else(|| {
            let start = self.clamp_boundary(start.min(self.len()));
            let end = self.clamp_boundary(end.min(self.len()));
            if end < start {
                return None;
            }
            let mut result = String::with_capacity(end.saturating_sub(start));
            let mut valid = true;
            let mut known_ascii = true;
            self.tree
                .visit_range(start, end, |chunk, local_start, local_end| {
                    if let Some(text) = chunk.as_str().get(local_start..local_end) {
                        result.push_str(text);
                        known_ascii &= chunk.storage_is_ascii || text.is_ascii();
                    } else {
                        valid = false;
                    }
                });
            if !valid || result.len() != end.saturating_sub(start) {
                return None;
            }
            let storage = Arc::new(result);
            Some(ChunkedTextSlice::new_with_ascii(
                storage.clone(),
                0,
                storage.len(),
                known_ascii,
            ))
        })
    }

    pub(crate) fn slice(&self, start: usize, end: usize) -> Option<Self> {
        let start = self.clamp_boundary(start.min(self.len()));
        let end = self.clamp_boundary(end.min(self.len()));
        if end < start {
            return None;
        }
        let (_, tail) = split_node(self.tree.root.clone(), start);
        let (root, _) = split_node(tail, end.saturating_sub(start));
        Some(Self {
            tree: PersistentSequence { root },
        })
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
            PersistentSequence::from_leaves(split_text_chunks_from_string(inserted.to_string())),
        );
        true
    }

    pub(crate) fn replace_range_shared(
        &mut self,
        start: usize,
        end: usize,
        inserted: Arc<String>,
    ) -> bool {
        let start = self.clamp_boundary(start.min(self.len()));
        let end = self.clamp_boundary(end.min(self.len()));
        if end < start {
            return false;
        }
        self.tree.replace_range(
            start,
            end,
            PersistentSequence::from_leaves(split_text_chunks_from_storage(inserted)),
        );
        true
    }

    pub(crate) fn replace_range_chunked(
        &mut self,
        start: usize,
        end: usize,
        inserted: Self,
    ) -> bool {
        let start = self.clamp_boundary(start.min(self.len()));
        let end = self.clamp_boundary(end.min(self.len()));
        if end < start {
            return false;
        }
        self.tree.replace_range(start, end, inserted.tree);
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

    pub(crate) fn collect_line_ends_in_range(
        &self,
        start: usize,
        end: usize,
        output: &mut Vec<usize>,
    ) {
        output.clear();
        let start = start.min(self.len());
        let end = end.min(self.len()).max(start);
        let mut logical_cursor = start;
        self.tree
            .visit_range(start, end, |chunk, local_start, local_end| {
                let absolute_start = chunk.start.saturating_add(local_start);
                let absolute_end = chunk.start.saturating_add(local_end).min(chunk.end);
                let first = chunk
                    .line_breaks()
                    .partition_point(|position| *position < absolute_start);
                let last = chunk
                    .line_breaks()
                    .partition_point(|position| *position < absolute_end);
                if let Some(line_breaks) = chunk.line_breaks().get(first..last) {
                    output.extend(line_breaks.iter().map(|position| {
                        logical_cursor
                            .saturating_add(position.saturating_sub(absolute_start))
                            .saturating_add(1)
                    }));
                }
                logical_cursor =
                    logical_cursor.saturating_add(local_end.saturating_sub(local_start));
            });
        if output.last().copied().unwrap_or(start) < end {
            output.push(end);
        }
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

#[derive(Clone, Debug)]
struct ValueChunk<T> {
    storage: Arc<Vec<T>>,
    start: usize,
    end: usize,
}

impl<T> ValueChunk<T> {
    fn from_storage(storage: Arc<Vec<T>>, start: usize, end: usize) -> Self {
        let start = start.min(storage.len());
        let end = end.min(storage.len()).max(start);
        Self {
            storage,
            start,
            end,
        }
    }

    fn as_slice(&self) -> &[T] {
        self.storage.get(self.start..self.end).unwrap_or_default()
    }

    #[cfg(test)]
    fn shares_storage_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.storage, &other.storage)
    }
}

impl<T: PartialEq> PartialEq for ValueChunk<T> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T: Eq> Eq for ValueChunk<T> {}

impl<T: Clone> SequenceLeaf for ValueChunk<T> {
    fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
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
        let absolute = self.start.saturating_add(position).min(self.end);
        let left = (absolute > self.start)
            .then(|| Self::from_storage(self.storage.clone(), self.start, absolute));
        let right = (absolute < self.end)
            .then(|| Self::from_storage(self.storage.clone(), absolute, self.end));
        (left, right)
    }

    fn rechunk_pair(left: &Self, right: &Self) -> Vec<Self> {
        if Arc::ptr_eq(&left.storage, &right.storage) && left.end == right.start {
            return vec![Self::from_storage(
                left.storage.clone(),
                left.start,
                right.end,
            )];
        }

        let combined_len = left.len().saturating_add(right.len());
        if combined_len > VALUE_CHUNK_TARGET_LEN
            && left.len() >= VALUE_CHUNK_RECHUNK_MIN_LEN
            && right.len() >= VALUE_CHUNK_RECHUNK_MIN_LEN
        {
            return vec![left.clone(), right.clone()];
        }
        if combined_len > VALUE_CHUNK_TARGET_LEN
            && (left.len() >= VALUE_CHUNK_TARGET_LEN || right.len() >= VALUE_CHUNK_TARGET_LEN)
        {
            return vec![left.clone(), right.clone()];
        }

        let mut combined = Vec::with_capacity(left.len().saturating_add(right.len()));
        combined.extend_from_slice(left.as_slice());
        combined.extend_from_slice(right.as_slice());
        value_chunks(combined)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ChunkedValues<T> {
    tree: PersistentSequence<ValueChunk<T>>,
}

impl<T: Clone> ChunkedValues<T> {
    #[cfg(test)]
    pub(crate) fn from_vec(values: Vec<T>) -> Self {
        Self {
            tree: PersistentSequence::from_leaves(value_chunks(values)),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.tree.len()
    }

    pub(crate) fn get(&self, position: usize) -> Option<&T> {
        let (chunk, offset) = self.tree.locate(position)?;
        chunk.as_slice().get(offset)
    }

    #[cfg(test)]
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
                if let Some(values) = chunk.as_slice().get(local_start..local_end) {
                    result.extend_from_slice(values);
                } else {
                    valid = false;
                }
            });
        (valid && result.len() == end.saturating_sub(start)).then_some(result)
    }

    pub(crate) fn range_matches_slice(&self, start: usize, end: usize, expected: &[T]) -> bool
    where
        T: PartialEq,
    {
        let start = start.min(self.len());
        let end = end.min(self.len());
        if end < start || end.saturating_sub(start) != expected.len() {
            return false;
        }
        let mut expected_start = 0usize;
        let mut matches = true;
        self.tree
            .visit_range(start, end, |chunk, local_start, local_end| {
                if !matches {
                    return;
                }
                let Some(values) = chunk.as_slice().get(local_start..local_end) else {
                    matches = false;
                    return;
                };
                let expected_end = expected_start.saturating_add(values.len());
                matches = expected.get(expected_start..expected_end) == Some(values);
                expected_start = expected_end;
            });
        matches && expected_start == expected.len()
    }

    pub(crate) fn first_matching_position<F>(
        &self,
        start: usize,
        end: usize,
        mut predicate: F,
    ) -> Option<usize>
    where
        F: FnMut(&T) -> bool,
    {
        let start = start.min(self.len());
        let end = end.min(self.len());
        if end <= start {
            return None;
        }
        let mut found = None;
        let mut absolute = start;
        self.tree
            .visit_range(start, end, |chunk, local_start, local_end| {
                if found.is_some() {
                    return;
                }
                let Some(values) = chunk.as_slice().get(local_start..local_end) else {
                    return;
                };
                if let Some(relative) = values.iter().position(&mut predicate) {
                    found = Some(absolute.saturating_add(relative));
                }
                absolute = absolute.saturating_add(values.len());
            });
        found
    }

    pub(crate) fn replace_range(&mut self, start: usize, end: usize, replacement: Vec<T>) {
        self.tree.replace_range(
            start,
            end,
            PersistentSequence::from_leaves(value_chunks(replacement)),
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct ValueRun<T> {
    value: T,
    len: usize,
}

impl<T: Clone + Eq> SequenceLeaf for ValueRun<T> {
    fn len(&self) -> usize {
        self.len
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
        let position = position.min(self.len);
        let left = (position > 0).then(|| Self {
            value: self.value.clone(),
            len: position,
        });
        let right_len = self.len.saturating_sub(position);
        let right = (right_len > 0).then(|| Self {
            value: self.value.clone(),
            len: right_len,
        });
        (left, right)
    }

    fn rechunk_pair(left: &Self, right: &Self) -> Vec<Self> {
        if left.value == right.value {
            vec![Self {
                value: left.value.clone(),
                len: left.len.saturating_add(right.len),
            }]
        } else {
            vec![left.clone(), right.clone()]
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RunValues<T> {
    tree: PersistentSequence<ValueRun<T>>,
}

impl<T: Clone + Eq> RunValues<T> {
    #[cfg(test)]
    pub(crate) fn from_vec(values: Vec<T>) -> Self {
        let mut runs = Vec::<ValueRun<T>>::new();
        for value in values {
            if let Some(last) = runs.last_mut().filter(|run| run.value == value) {
                last.len = last.len.saturating_add(1);
            } else {
                runs.push(ValueRun { value, len: 1 });
            }
        }
        Self {
            tree: PersistentSequence::from_leaves(runs),
        }
    }

    pub(crate) fn from_runs(runs: Vec<(T, usize)>) -> Self {
        let mut normalized = Vec::<ValueRun<T>>::new();
        for (value, len) in runs {
            if len == 0 {
                continue;
            }
            if let Some(last) = normalized.last_mut().filter(|run| run.value == value) {
                last.len = last.len.saturating_add(len);
            } else {
                normalized.push(ValueRun { value, len });
            }
        }
        Self {
            tree: PersistentSequence::from_leaves(normalized),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.tree.len()
    }

    pub(crate) fn get(&self, position: usize) -> Option<&T> {
        let (run, _) = self.tree.locate(position)?;
        Some(&run.value)
    }

    pub(crate) fn last(&self) -> Option<&T> {
        self.len()
            .checked_sub(1)
            .and_then(|position| self.get(position))
    }

    pub(crate) fn replace_range_runs(
        &mut self,
        start: usize,
        end: usize,
        replacement: Vec<(T, usize)>,
    ) {
        self.tree
            .replace_range(start, end, Self::from_runs(replacement).tree);
    }

    #[cfg(test)]
    pub(crate) fn run_count(&self) -> usize {
        self.tree.leaf_count()
    }
}

fn value_chunks<T>(values: Vec<T>) -> Vec<ValueChunk<T>> {
    if values.is_empty() {
        return Vec::new();
    }
    let storage = Arc::new(values);
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < storage.len() {
        let end = start
            .saturating_add(VALUE_CHUNK_TARGET_LEN)
            .min(storage.len());
        chunks.push(ValueChunk::from_storage(storage.clone(), start, end));
        start = end;
    }
    chunks
}

fn split_text_chunks_from_string(text: String) -> Vec<TextChunk> {
    split_text_chunks_from_storage(Arc::new(text))
}

fn split_text_chunks_from_storage(storage: Arc<String>) -> Vec<TextChunk> {
    if storage.is_empty() {
        return Vec::new();
    }
    let (line_breaks, storage_is_ascii) = text_storage_metadata(storage.as_bytes());
    let storage_line_breaks: Arc<[usize]> = line_breaks.into();
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < storage.len() {
        let mut end = start
            .saturating_add(TEXT_CHUNK_TARGET_BYTES)
            .min(storage.len());
        while end > start && !storage.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            end = storage[start..]
                .char_indices()
                .nth(1)
                .map(|(idx, _)| start + idx)
                .unwrap_or(storage.len());
        }
        if end < storage.len()
            && storage.as_bytes().get(end.saturating_sub(1)) == Some(&b'\r')
            && storage.as_bytes().get(end) == Some(&b'\n')
        {
            end += 1;
        }
        chunks.push(TextChunk::from_storage(
            storage.clone(),
            storage_line_breaks.clone(),
            storage_is_ascii,
            start,
            end,
        ));
        start = end;
    }
    chunks
}

fn text_storage_metadata(bytes: &[u8]) -> (Vec<usize>, bool) {
    let mut positions = Vec::new();
    let mut is_ascii = true;
    let mut idx = 0usize;
    while idx < bytes.len() {
        is_ascii &= bytes[idx].is_ascii();
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
    (positions, is_ascii)
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
    fn large_text_and_value_chunks_share_one_input_allocation() {
        let shared_text = Arc::new("SELECT 한글;\n".repeat(20_000));
        let mut text = ChunkedText::default();
        assert!(text.replace_range_shared(0, 0, shared_text.clone()));
        let mut text_chunk_count = 0usize;
        text.tree.for_each_leaf(|chunk| {
            text_chunk_count = text_chunk_count.saturating_add(1);
            assert!(Arc::ptr_eq(&chunk.storage, &shared_text));
        });
        assert!(text_chunk_count > 1);

        let values = ChunkedValues::from_vec((0..20_000).collect::<Vec<_>>());
        let mut value_chunks = Vec::new();
        values
            .tree
            .for_each_leaf(|chunk| value_chunks.push(chunk.clone()));
        assert!(value_chunks.len() > 1);
        assert!(value_chunks
            .iter()
            .skip(1)
            .all(|chunk| chunk.shares_storage_with(&value_chunks[0])));
    }

    #[test]
    fn chunked_text_line_queries_match_flat_text() {
        let source = "a\r\nb\nc\rd".repeat(10_000);
        let text = ChunkedText::from_str(&source);
        let (breaks, _) = text_storage_metadata(source.as_bytes());
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
    fn shared_ranges_and_line_ends_reuse_one_large_paste_allocation() {
        let prefix = Arc::new("head\n".to_string());
        let pasted = Arc::new("SELECT 1;\r\n".repeat(20_000));
        let mut text = ChunkedText::from_shared_string(prefix.clone());
        let paste_start = text.len();
        assert!(text.replace_range_shared(paste_start, paste_start, pasted.clone()));

        let paste_end = paste_start.saturating_add(pasted.len());
        let shared = text
            .shared_range(paste_start, paste_end)
            .expect("the pasted range must remain one shared allocation");
        let original = ChunkedTextSlice::whole(pasted.clone());
        assert!(shared.shares_storage_with(&original));
        assert!(shared.is_known_ascii());

        let mut actual_line_ends = Vec::new();
        text.collect_line_ends_in_range(paste_start, paste_end, &mut actual_line_ends);
        let (breaks, _) = text_storage_metadata(pasted.as_bytes());
        let expected_line_ends = breaks
            .into_iter()
            .map(|position| paste_start.saturating_add(position).saturating_add(1))
            .collect::<Vec<_>>();
        assert_eq!(actual_line_ends, expected_line_ends);
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
    fn run_values_keep_a_million_equal_line_states_in_one_run() {
        let mut values = RunValues::from_runs(vec![(0u8, 1_000_001)]);
        assert_eq!(values.len(), 1_000_001);
        assert_eq!(values.run_count(), 1);
        assert_eq!(values.get(500_000), Some(&0));

        values.replace_range_runs(500_000, 500_001, vec![(1, 1)]);

        assert_eq!(values.len(), 1_000_001);
        assert_eq!(values.get(499_999), Some(&0));
        assert_eq!(values.get(500_000), Some(&1));
        assert_eq!(values.get(500_001), Some(&0));
        assert_eq!(values.run_count(), 3);
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
