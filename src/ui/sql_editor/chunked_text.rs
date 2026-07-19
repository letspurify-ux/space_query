use std::sync::Arc;

const TEXT_CHUNK_TARGET_BYTES: usize = 32 * 1024;
const VALUE_CHUNK_TARGET_LEN: usize = 4 * 1024;

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

    fn len(&self) -> usize {
        self.text.len()
    }
}

/// Persistent chunked UTF-8 text used by editor-side mirrors and undo snapshots.
///
/// Clones share every chunk. An edit copies only the chunks touching the edited
/// range and shifts a small vector of chunk descriptors, so the cost is bounded
/// by the edited text and the number of chunks rather than the document tail.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ChunkedText {
    chunks: Vec<TextChunk>,
    len: usize,
    line_break_count: usize,
}

impl ChunkedText {
    pub(crate) fn from_string(text: String) -> Self {
        Self::from_str(&text)
    }

    pub(crate) fn from_str(text: &str) -> Self {
        let chunks = split_text_chunks(text);
        let line_break_count = chunks.iter().map(|chunk| chunk.line_breaks.len()).sum();
        Self {
            chunks,
            len: text.len(),
            line_break_count,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[cfg(test)]
    pub(crate) fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    #[cfg(test)]
    pub(crate) fn shared_chunk_count(&self, other: &Self) -> usize {
        self.chunks
            .iter()
            .filter(|left| {
                other
                    .chunks
                    .iter()
                    .any(|right| Arc::ptr_eq(&left.text, &right.text))
            })
            .count()
    }

    #[cfg(test)]
    pub(crate) fn starts_with(&self, prefix: impl ToString) -> bool {
        let prefix = prefix.to_string();
        self.range_string(0, prefix.len())
            .is_some_and(|head| head == prefix)
    }

    pub(crate) fn to_flat_string(&self) -> String {
        let mut text = String::with_capacity(self.len);
        for chunk in &self.chunks {
            text.push_str(&chunk.text);
        }
        text
    }

    pub(crate) fn byte_at(&self, pos: usize) -> Option<u8> {
        if pos >= self.len {
            return None;
        }
        let (idx, offset) = self.locate(pos);
        self.chunks
            .get(idx)
            .and_then(|chunk| chunk.text.as_bytes().get(offset))
            .copied()
    }

    pub(crate) fn is_char_boundary(&self, pos: usize) -> bool {
        if pos == 0 || pos == self.len {
            return true;
        }
        self.byte_at(pos)
            .is_some_and(|byte| byte & 0b1100_0000 != 0b1000_0000)
    }

    pub(crate) fn clamp_boundary(&self, pos: usize) -> usize {
        let mut pos = pos.min(self.len);
        while pos > 0 && !self.is_char_boundary(pos) {
            pos -= 1;
        }
        pos
    }

    pub(crate) fn range_string(&self, start: usize, end: usize) -> Option<String> {
        let start = self.clamp_boundary(start.min(self.len));
        let end = self.clamp_boundary(end.min(self.len));
        if end < start {
            return Some(String::new());
        }
        let mut result = String::with_capacity(end.saturating_sub(start));
        let mut chunk_start = 0usize;
        for chunk in &self.chunks {
            let chunk_end = chunk_start.saturating_add(chunk.len());
            if chunk_end <= start {
                chunk_start = chunk_end;
                continue;
            }
            if chunk_start >= end {
                break;
            }
            let local_start = start.saturating_sub(chunk_start).min(chunk.len());
            let local_end = end.saturating_sub(chunk_start).min(chunk.len());
            result.push_str(chunk.text.get(local_start..local_end)?);
            chunk_start = chunk_end;
        }
        Some(result)
    }

    pub(crate) fn replace_range(&mut self, start: usize, end: usize, inserted: &str) -> bool {
        let start = self.clamp_boundary(start.min(self.len));
        let end = self.clamp_boundary(end.min(self.len));
        if end < start {
            return false;
        }

        let (mut start_idx, mut start_offset) = self.locate(start);
        let (end_idx, end_offset) = self.locate(end);
        // Include the left neighbor when an edit starts exactly on a chunk
        // boundary. Besides keeping chunks balanced, this prevents an inserted
        // `\n` from forming a CRLF pair with an untouched trailing `\r` whose
        // two bytes would otherwise live in separate metadata domains.
        if start_offset == 0 && start_idx > 0 {
            start_idx = start_idx.saturating_sub(1);
            start_offset = self.chunks.get(start_idx).map_or(0, TextChunk::len);
        }
        let mut replacement = String::new();
        if let Some(chunk) = self.chunks.get(start_idx) {
            let Some(prefix) = chunk.text.get(..start_offset) else {
                return false;
            };
            replacement.push_str(prefix);
        }
        replacement.push_str(inserted);
        if let Some(chunk) = self.chunks.get(end_idx) {
            let Some(suffix) = chunk.text.get(end_offset..) else {
                return false;
            };
            replacement.push_str(suffix);
        }

        let drain_start = start_idx.min(self.chunks.len());
        let drain_end = if end_idx < self.chunks.len() {
            end_idx.saturating_add(1)
        } else {
            self.chunks.len()
        };
        let removed_line_breaks = self.chunks[drain_start..drain_end.max(drain_start)]
            .iter()
            .map(|chunk| chunk.line_breaks.len())
            .sum::<usize>();
        let replacement_chunks = split_text_chunks(&replacement);
        let replacement_line_breaks = replacement_chunks
            .iter()
            .map(|chunk| chunk.line_breaks.len())
            .sum::<usize>();
        self.chunks
            .splice(drain_start..drain_end.max(drain_start), replacement_chunks);
        self.len = self
            .len
            .saturating_sub(end.saturating_sub(start))
            .saturating_add(inserted.len());
        self.line_break_count = self
            .line_break_count
            .saturating_sub(removed_line_breaks)
            .saturating_add(replacement_line_breaks);
        true
    }

    pub(crate) fn line_count(&self) -> usize {
        if self.is_empty() {
            0
        } else {
            self.line_break_count.saturating_add(1)
        }
    }

    pub(crate) fn line_index_for_position(&self, pos: usize) -> usize {
        let pos = pos.min(self.len);
        let mut absolute = 0usize;
        let mut line_index = 0usize;
        for chunk in &self.chunks {
            let chunk_end = absolute.saturating_add(chunk.len());
            if pos < chunk_end {
                let relative = pos.saturating_sub(absolute);
                line_index += chunk.line_breaks.partition_point(|value| *value < relative);
                return line_index;
            }
            line_index = line_index.saturating_add(chunk.line_breaks.len());
            absolute = chunk_end;
        }
        line_index
    }

    pub(crate) fn line_start(&self, pos: usize) -> usize {
        self.line_start_for_index(self.line_index_for_position(pos))
    }

    pub(crate) fn line_end(&self, pos: usize) -> usize {
        self.nth_line_break(self.line_index_for_position(pos))
            .unwrap_or(self.len)
    }

    pub(crate) fn line_start_for_index(&self, line_index: usize) -> usize {
        if line_index == 0 {
            return 0;
        }
        self.nth_line_break(line_index.saturating_sub(1))
            .map(|value| value.saturating_add(1))
            .unwrap_or(self.len)
    }

    pub(crate) fn inclusive_line_end_for_index(&self, line_index: usize) -> usize {
        self.nth_line_break(line_index)
            .map(|value| value.saturating_add(1).min(self.len))
            .unwrap_or(self.len)
    }

    fn nth_line_break(&self, mut ordinal: usize) -> Option<usize> {
        let mut absolute = 0usize;
        for chunk in &self.chunks {
            if ordinal < chunk.line_breaks.len() {
                return chunk
                    .line_breaks
                    .get(ordinal)
                    .map(|value| absolute.saturating_add(*value));
            }
            ordinal = ordinal.saturating_sub(chunk.line_breaks.len());
            absolute = absolute.saturating_add(chunk.len());
        }
        None
    }

    fn locate(&self, pos: usize) -> (usize, usize) {
        let pos = pos.min(self.len);
        let mut absolute = 0usize;
        for (idx, chunk) in self.chunks.iter().enumerate() {
            let chunk_end = absolute.saturating_add(chunk.len());
            if pos < chunk_end {
                return (idx, pos.saturating_sub(absolute));
            }
            if pos == chunk_end {
                return (idx.saturating_add(1), 0);
            }
            absolute = chunk_end;
        }
        (self.chunks.len(), 0)
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
        self.len == other.len() && self.to_flat_string() == *other
    }
}

impl PartialEq<String> for ChunkedText {
    fn eq(&self, other: &String) -> bool {
        self.len == other.len() && self.to_flat_string() == *other
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ChunkedValues<T> {
    chunks: Vec<Arc<[T]>>,
    len: usize,
}

impl<T: Clone> ChunkedValues<T> {
    pub(crate) fn from_vec(values: Vec<T>) -> Self {
        let len = values.len();
        let chunks = values
            .chunks(VALUE_CHUNK_TARGET_LEN)
            .map(|chunk| Arc::<[T]>::from(chunk.to_vec()))
            .collect();
        Self { chunks, len }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn get(&self, pos: usize) -> Option<&T> {
        let (chunk_idx, offset) = self.locate_existing(pos)?;
        self.chunks
            .get(chunk_idx)
            .and_then(|chunk| chunk.get(offset))
    }

    pub(crate) fn last(&self) -> Option<&T> {
        self.chunks.last().and_then(|chunk| chunk.last())
    }

    pub(crate) fn range_vec(&self, start: usize, end: usize) -> Option<Vec<T>> {
        let start = start.min(self.len);
        let end = end.min(self.len);
        if end < start {
            return None;
        }
        let mut result = Vec::with_capacity(end.saturating_sub(start));
        let mut chunk_start = 0usize;
        for chunk in &self.chunks {
            let chunk_end = chunk_start.saturating_add(chunk.len());
            if chunk_end <= start {
                chunk_start = chunk_end;
                continue;
            }
            if chunk_start >= end {
                break;
            }
            let local_start = start.saturating_sub(chunk_start).min(chunk.len());
            let local_end = end.saturating_sub(chunk_start).min(chunk.len());
            result.extend_from_slice(chunk.get(local_start..local_end)?);
            chunk_start = chunk_end;
        }
        (result.len() == end.saturating_sub(start)).then_some(result)
    }

    pub(crate) fn set(&mut self, pos: usize, value: T) -> bool {
        let Some((chunk_idx, offset)) = self.locate_existing(pos) else {
            return false;
        };
        let mut chunk = self.chunks[chunk_idx].to_vec();
        let Some(slot) = chunk.get_mut(offset) else {
            return false;
        };
        *slot = value;
        self.chunks[chunk_idx] = chunk.into();
        true
    }

    pub(crate) fn replace_range(&mut self, start: usize, end: usize, replacement: Vec<T>) {
        let start = start.min(self.len);
        let end = end.min(self.len).max(start);
        let replacement_len = replacement.len();
        let (start_idx, start_offset) = self.locate_boundary(start);
        let (end_idx, end_offset) = self.locate_boundary(end);
        let mut merged = Vec::new();
        if let Some(chunk) = self.chunks.get(start_idx) {
            merged.extend_from_slice(&chunk[..start_offset]);
        }
        merged.extend(replacement);
        if let Some(chunk) = self.chunks.get(end_idx) {
            merged.extend_from_slice(&chunk[end_offset..]);
        }
        let drain_start = start_idx.min(self.chunks.len());
        let drain_end = if end_idx < self.chunks.len() {
            end_idx.saturating_add(1)
        } else {
            self.chunks.len()
        };
        let next_chunks = merged
            .chunks(VALUE_CHUNK_TARGET_LEN)
            .map(|chunk| Arc::<[T]>::from(chunk.to_vec()))
            .collect::<Vec<_>>();
        self.chunks
            .splice(drain_start..drain_end.max(drain_start), next_chunks);
        self.len = self
            .len
            .saturating_sub(end.saturating_sub(start))
            .saturating_add(replacement_len);
    }

    pub(crate) fn resize(&mut self, new_len: usize, value: T) {
        if new_len < self.len {
            self.replace_range(new_len, self.len, Vec::new());
        } else if new_len > self.len {
            self.replace_range(self.len, self.len, vec![value; new_len - self.len]);
        }
    }

    pub(crate) fn truncate(&mut self, len: usize) {
        if len < self.len {
            self.replace_range(len, self.len, Vec::new());
        }
    }

    #[cfg(test)]
    pub(crate) fn to_vec(&self) -> Vec<T> {
        self.chunks
            .iter()
            .flat_map(|chunk| chunk.iter().cloned())
            .collect()
    }

    fn locate_existing(&self, pos: usize) -> Option<(usize, usize)> {
        if pos >= self.len {
            return None;
        }
        let mut absolute = 0usize;
        for (idx, chunk) in self.chunks.iter().enumerate() {
            let end = absolute.saturating_add(chunk.len());
            if pos < end {
                return Some((idx, pos.saturating_sub(absolute)));
            }
            absolute = end;
        }
        None
    }

    fn locate_boundary(&self, pos: usize) -> (usize, usize) {
        let pos = pos.min(self.len);
        let mut absolute = 0usize;
        for (idx, chunk) in self.chunks.iter().enumerate() {
            let end = absolute.saturating_add(chunk.len());
            if pos < end {
                return (idx, pos.saturating_sub(absolute));
            }
            if pos == end {
                return (idx.saturating_add(1), 0);
            }
            absolute = end;
        }
        (self.chunks.len(), 0)
    }
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
        let chunks_before = text.chunks.clone();
        let edit_at = source.find('\n').unwrap_or(0) + 1;
        assert!(text.replace_range(edit_at, edit_at, "SELECT 1;\n"));
        let flattened = text.to_flat_string();
        assert_eq!(
            flattened,
            format!("{}SELECT 1;\n{}", &source[..edit_at], &source[edit_at..])
        );
        assert!(
            chunks_before.iter().skip(2).any(|before| text
                .chunks
                .iter()
                .any(|after| Arc::ptr_eq(&before.text, &after.text))),
            "unaffected tail chunks should remain shared"
        );
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
    fn chunked_values_replaces_only_boundary_chunks() {
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
