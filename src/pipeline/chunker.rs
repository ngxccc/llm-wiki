use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use regex::Regex;
use std::sync::OnceLock;

/// Compile Regex once per application lifecycle to save CPU cycle.
fn base64_img_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"!\[.*?\]\(data:image/[^;]+;base64,[^\)]+\)")
            .expect("invalid base64 image regex")
    })
}

fn html_comment_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?s)<!--.*?-->")
            .expect("CRITICAL: Failed to compile HTML comment Regex. Check your syntax!")
    })
}

/// Strips out Base64 images and HTML comments before AST parsing.
/// Time Complexity: O(N) where N is text length. Space: O(N) for string replacement.
fn clean_markdown(text: &str) -> String {
    let no_images = base64_img_regex().replace_all(text, "[IMAGE REMOVED]");
    let no_comments = html_comment_regex().replace_all(&no_images, "");
    no_comments.into_owned()
}

/// Chops oversized paragraphs by sentences, keeping 1 sentence overlap.
/// Time Complexity: O(M) where M is paragraph length. Space: O(K) for chunks.
fn semantic_chunk_paragraph(text: &str, max_bytes: usize) -> Vec<String> {
    let sentences: Vec<&str> = text.split_inclusive(&['.', '!', '?', '\n'][..]).collect();
    let mut chunks = Vec::new();
    let mut current_chunk = String::new();
    let mut last_sentence = String::new();

    for sentence in sentences {
        let clean_sentence = sentence.trim();
        if clean_sentence.is_empty() {
            continue;
        }

        // Survival fallback for giant sentences (e.g. minified JSON line)
        if clean_sentence.len() > max_bytes {
            if !current_chunk.is_empty() {
                chunks.push(current_chunk.trim().to_string());
                current_chunk.clear();
            }
            chunks.extend(byte_slice_fallback(clean_sentence, max_bytes));
            continue;
        }

        if current_chunk.len() + clean_sentence.len() > max_bytes && !current_chunk.is_empty() {
            chunks.push(current_chunk.trim().to_string());
            current_chunk.clear();
            // Overlap: Prepend the last sentence to the new chunk
            if !last_sentence.is_empty() {
                current_chunk.push_str(&last_sentence);
                current_chunk.push(' ');
            }
        }

        current_chunk.push_str(clean_sentence);
        current_chunk.push(' ');
        last_sentence = clean_sentence.to_string();
    }

    if !current_chunk.trim().is_empty() {
        chunks.push(current_chunk.trim().to_string());
    }
    chunks
}

/// Helper: Absolute survival mode for byte-slicing (UTF-8 safe)
fn byte_slice_fallback(text: &str, max_bytes: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + max_bytes).min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(text[start..end].to_string());
        start = end;
    }
    chunks
}

/// Parses Markdown AST, preserves `CodeBlock`s, and routes paragraphs to semantic chopper.
/// Time Complexity: O(N) single pass parsing. Space Complexity: O(N) for chunks.
pub fn ultimate_markdown_chunker(text: &str, max_bytes: usize) -> Vec<String> {
    let cleaned_text = clean_markdown(text);

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(&cleaned_text, options);

    let mut chunks = Vec::new();
    let mut current_block = String::new();
    let mut in_code_block = false;

    let mut in_table = false;
    let mut in_table_head = false;
    let mut header_start_idx = 0;
    let mut header_buffer = String::new();

    // Traverse the Abstract Syntax Tree (AST)
    for event in parser {
        match event {
            Event::Start(Tag::Table(_)) => {
                in_table = true;
                header_buffer.clear();
                // Thêm \n trước bảng cho đẹp nếu block chưa có
                if !current_block.is_empty() && !current_block.ends_with('\n') {
                    current_block.push('\n');
                }
            }
            Event::End(TagEnd::Table) => {
                in_table = false;
                header_buffer.clear();
                // Kết thúc bảng, nếu dung lượng quá lớn thì chốt chunk luôn
                if current_block.len() > max_bytes {
                    chunks.extend(semantic_chunk_paragraph(&current_block, max_bytes));
                    current_block.clear();
                } else if !current_block.trim().is_empty() {
                    chunks.push(current_block.trim().to_string());
                    current_block.clear();
                }
            }

            Event::Start(Tag::TableHead) => {
                in_table_head = true;
                header_start_idx = current_block.len(); // Đánh dấu vị trí bắt đầu
            }
            Event::End(TagEnd::TableHead) => {
                in_table_head = false;
                current_block.push('\n');
                // Copy chính xác chuỗi text của Header vừa được tạo ra
                if header_start_idx <= current_block.len() {
                    header_buffer = current_block[header_start_idx..].to_string();
                }
            }

            Event::Start(Tag::TableCell) => current_block.push_str("| "),
            Event::End(TagEnd::TableCell) => current_block.push(' '),

            Event::End(TagEnd::TableRow) => {
                current_block.push('\n'); // Xuống dòng sau mỗi hàng

                // NẾU: Đang ở trong bảng + Không phải là header + Đã tràn max_bytes
                if in_table && !in_table_head && current_block.len() > max_bytes {
                    chunks.push(current_block.trim().to_string());
                    current_block.clear();
                    current_block.push_str(&header_buffer);
                }
            }

            // Entered a Code Block
            Event::Start(Tag::CodeBlock(_)) => {
                in_code_block = true;
                current_block.push_str("```\n");
            }
            // Exited a Code Block -> Push it entirely (Bypass semantic fallback)
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;

                if !current_block.ends_with('\n') {
                    current_block.push('\n');
                }
                current_block.push_str("```");

                let hard_limit = max_bytes.max(8192);

                // Graceful Degradation: Even code blocks must be chopped if they are > max_bytes
                if current_block.len() > hard_limit {
                    chunks.extend(byte_slice_fallback(&current_block, max_bytes));
                } else {
                    chunks.push(current_block.clone());
                }
                current_block.clear();
            }

            // Catch raw text or code snippets
            Event::Text(t) | Event::Code(t) => {
                current_block.push_str(&t);
            }
            Event::SoftBreak | Event::HardBreak => {
                current_block.push('\n');
            }

            // Exited a Paragraph or List Item
            Event::End(TagEnd::Paragraph | TagEnd::Item) => {
                if !in_code_block {
                    if current_block.len() > max_bytes {
                        // Route to TIER 3: Semantic Chopper
                        chunks.extend(semantic_chunk_paragraph(&current_block, max_bytes));
                    } else if !current_block.trim().is_empty() {
                        chunks.push(current_block.trim().to_string());
                    }
                    current_block.clear();
                }
            }
            _ => {}
        }
    }

    // Flush any remaining text in buffer
    if !current_block.trim().is_empty() {
        if current_block.len() > max_bytes {
            chunks.extend(semantic_chunk_paragraph(&current_block, max_bytes));
        } else {
            chunks.push(current_block.trim().to_string());
        }
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier2_ast_table_header_injection() {
        let md_table = "
| Employee | Role | Salary |
|---|---|---|
| Alice | Dev | 1000 |
| Bob | PM | 1500 |
| Charlie | Design | 1200 |
";
        // Ép max_bytes cực thấp (25 bytes) để nó bắt buộc phải đứt sau hàng của Alice
        let chunks = ultimate_markdown_chunker(md_table, 25);

        // Chunk 1: Header + Dòng của Alice
        assert!(chunks[0].contains("Employee | Role | Salary"));
        assert!(chunks[0].contains("Alice"));

        // Chunk 2: VẪN CÓ HEADER + Dòng của Bob
        assert!(chunks[1].contains("Employee | Role | Salary"));
        assert!(chunks[1].contains("Bob"));

        // Chunk 3: VẪN CÓ HEADER + Dòng của Charlie
        assert!(chunks[2].contains("Employee | Role | Salary"));
        assert!(chunks[2].contains("Charlie"));
    }

    #[test]
    fn test_tier1_filter_base64_and_html() {
        let dirty_text = "Hello World! \nHere is an image: ![architecture](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgA...) End.\n hello world";
        let chunks = ultimate_markdown_chunker(dirty_text, 1000);

        let result = &chunks[0];
        assert!(!result.contains("secret"));
        assert!(!result.contains("iVBORw0KGgo"));
        assert!(result.contains("[IMAGE REMOVED]"));
    }

    #[test]
    fn test_tier2_ast_preserves_code_blocks() {
        let md_text =
            "Paragraph 1.\n\n```rust\nfn main() {\n  println!(\"Hello\");\n}\n```\n\nParagraph 2.";
        // Ép max_bytes rất nhỏ (20) để ép nó phải cắt, nhưng Code Block KHÔNG được bị cắt vỡ.
        let chunks = ultimate_markdown_chunker(md_text, 20);

        assert_eq!(chunks[0], "Paragraph 1.");
        // Block code được gom chung, không quan tâm max_bytes (trừ khi nó to quá mức)
        assert_eq!(
            chunks[1],
            "```\nfn main() {\n  println!(\"Hello\");\n}\n```"
        );
        assert_eq!(chunks[2], "Paragraph 2.");
    }

    #[test]
    fn test_tier3_semantic_fallback_with_overlap() {
        // Một Paragraph siêu dài, buộc phải xuống Tầng 3 để cắt gối đầu
        let giant_paragraph = "Sentence one is here. Sentence two is here. Sentence three is here. Sentence four is here.";
        // max_bytes = 45 -> Vừa đủ nhét 2 câu vào 1 chunk
        let chunks = ultimate_markdown_chunker(giant_paragraph, 45);

        assert_eq!(chunks[0], "Sentence one is here. Sentence two is here.");
        // Gối đầu: Câu 2 ("Sentence two...") lặp lại ở chunk tiếp theo!
        assert_eq!(chunks[1], "Sentence two is here. Sentence three is here.");
        assert_eq!(chunks[2], "Sentence three is here. Sentence four is here.");
    }

    #[test]
    fn test_ultimate_survival_utf8_boundary() {
        // Cục rác dính liền không có dấu chấm/phẩy, lại còn chứa Emoji (4 bytes / ký tự)
        // 🚀🚀🚀🚀🚀 = 20 bytes
        let emoji_spam = "🚀🚀🚀🚀🚀";

        // Cắt ở max_bytes = 6.
        // 6 bytes không chia hết cho 4 bytes. Thuật toán fallback phải lùi về 4 bytes để cắt an toàn.
        let chunks = byte_slice_fallback(emoji_spam, 6);

        // Chạy qua không bị Panic (sập server) là thành công rực rỡ!
        assert_eq!(chunks[0], "🚀"); // 4 bytes
        assert_eq!(chunks[1], "🚀");
    }

    #[test]
    fn test_giant_sentence_no_separator() {
        let text = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let chunks = ultimate_markdown_chunker(text, 10);

        assert!(chunks.len() > 1);
    }
    #[test]

    fn test_multiple_code_blocks() {
        let md = "```\na\n```\n\n```\nb\n```";

        // Ép max_bytes nhỏ để kiểm tra xem thẻ VIP của CodeBlock có hoạt động không
        let chunks = ultimate_markdown_chunker(md, 10);

        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].contains("```\na\n```"));
        assert!(chunks[1].contains("```\nb\n```"));
    }

    #[test]
    fn test_mixed_real_world() {
        let md = r"
Hello world.

| A | B |
|---|---|
| 1 | 2 |

```rust
fn main(){}
```
Another paragraph.
";
        let chunks = ultimate_markdown_chunker(md, 50);

        assert!(chunks
            .iter()
            .any(|c| c.contains("| A ") && c.contains("| B ")));
        assert!(chunks.iter().any(|c| c.contains("fn main")));
    }

    #[test]
    fn test_zero_or_small_limit() {
        let chunks = ultimate_markdown_chunker("Hello world", 1);
        assert!(!chunks.is_empty());
    }
}
