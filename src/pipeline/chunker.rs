/// Smart Semantic Chunker: Splits text by sentence boundaries (., !, ?)
/// while strictly respecting `max_bytes` and maintaining `overlap_sentences`
/// to prevent context loss in RAG pipelines.
///
/// Time: O(N) where N is bytes length.
/// Space: O(K) for chunks allocation.
pub fn semantic_chunk(text: &str, max_bytes: usize, overlap_sentences: usize) -> Vec<String> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    // 1. Split text into raw sentences aggressively including newlines
    let raw_sentences: Vec<&str> = text.split_inclusive(&['.', '!', '?', '\n'][..]).collect();

    let mut chunks = Vec::new();
    let mut current_chunk = String::new();

    // Time Complexity for buffer operations: O(1)
    // Space Complexity: O(overlap_sentences) pointers, ZERO heap allocation for the text itself!
    let mut sentence_buffer: Vec<&str> = Vec::new();

    for sentence in raw_sentences {
        let clean_sentence = sentence.trim_start();
        if clean_sentence.is_empty() {
            continue;
        }

        // Edge Case Handling: A single sentence is larger than max_bytes!
        // Fallback to strict byte-slicing to avoid blowing up the LLM context.
        if clean_sentence.len() > max_bytes {
            if !current_chunk.is_empty() {
                chunks.push(current_chunk.trim().to_string());
                current_chunk.clear();
                sentence_buffer.clear();
            }
            // Naive strict slice (in a real prod, we might split by spaces)
            let mut start = 0;
            while start < clean_sentence.len() {
                let end = (start + max_bytes).min(clean_sentence.len());
                // Ensure we don't slice inside a UTF-8 character boundary
                let mut safe_end = end;
                while !clean_sentence.is_char_boundary(safe_end) {
                    safe_end -= 1;
                }
                chunks.push(clean_sentence[start..safe_end].to_string());
                start = safe_end;
            }
            continue;
        }

        // Normal Case: Check if adding this sentence exceeds the limit
        if current_chunk.len() + clean_sentence.len() > max_bytes && !current_chunk.is_empty() {
            chunks.push(current_chunk.trim().to_string());
            // Build the next chunk starting with overlapping sentences
            current_chunk.clear();

            let overlap_start = sentence_buffer.len().saturating_sub(overlap_sentences);

            for &s in &sentence_buffer[overlap_start..] {
                current_chunk.push_str(s);
                current_chunk.push(' ');
            }
            sentence_buffer.clear(); // Reset buffer after copying overlap
        }

        current_chunk.push_str(clean_sentence);
        current_chunk.push(' ');
        sentence_buffer.push(clean_sentence);
    }

    // Push the remaining buffer
    if !current_chunk.trim().is_empty() {
        chunks.push(current_chunk.trim().to_string());
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::semantic_chunk;

    #[test]
    fn test_normal_semantic_split() {
        let text = "Hello world. My name is Raizo! How are you?";
        // Max 25 bytes per chunk, overlap 0
        let chunks = semantic_chunk(text, 25, 0);
        // "Hello world." (12 bytes) + " My name is Raizo!" (18 bytes) = 30 > 25.
        // So it should split.
        assert_eq!(chunks[0], "Hello world.");
        assert_eq!(chunks[1], "My name is Raizo!");
        assert_eq!(chunks[2], "How are you?");
    }

    #[test]
    fn test_overlap_sentences() {
        let text = "First sentence. Second sentence. Third sentence.";
        // Force split after 2nd sentence, overlap 1 sentence
        let chunks = semantic_chunk(text, 35, 1);
        assert_eq!(chunks[0], "First sentence. Second sentence.");
        assert_eq!(chunks[1], "Second sentence. Third sentence."); // Second sentence is repeated!
    }

    #[test]
    fn test_giant_sentence_fallback() {
        let text =
            "Short. A_very_long_sentence_that_exceeds_the_byte_limit_without_any_punctuation. End.";
        let chunks = semantic_chunk(text, 40, 0);
        assert_eq!(chunks[0], "Short.");
        // The giant string is strictly chopped
        assert_eq!(chunks[1], "A_very_long_sentence_that_exceeds_the_by");
        assert_eq!(chunks[2], "te_limit_without_any_punctuation.");
        assert_eq!(chunks[3], "End.");
    }
}
