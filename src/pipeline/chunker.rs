/// Splits markdown into overlapping chunks.
///
/// Time: O(n) over input bytes.
/// Space: O(k) for produced chunk strings.
pub fn chunk_markdown(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }

    let size = chunk_size.max(1);
    let stride = size.saturating_sub(overlap).max(1);
    let mut chunks = Vec::new();
    let mut start = 0_usize;
    let bytes = text.as_bytes();

    while start < bytes.len() {
        let mut end = (start + size).min(bytes.len());
        while end < bytes.len() && !text.is_char_boundary(end) {
            end -= 1;
        }

        if end <= start {
            break;
        }

        chunks.push(text[start..end].to_string());
        if end == bytes.len() {
            break;
        }

        let mut next_start = (start + stride).min(bytes.len());
        while next_start < bytes.len() && !text.is_char_boundary(next_start) {
            next_start += 1;
        }
        start = next_start;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::chunk_markdown;

    #[test]
    fn creates_overlapping_chunks() {
        let chunks = chunk_markdown("abcdefghij", 4, 2);
        assert_eq!(chunks, vec!["abcd", "cdef", "efgh", "ghij"]);
    }
}
