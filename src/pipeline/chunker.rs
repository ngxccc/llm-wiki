use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use regex::Regex;
use std::sync::OnceLock;

struct ChunkerContext {
    max_bytes: usize,
    chunks: Vec<String>,
    current_block: String,

    // State cho Code Block
    in_code_block: bool,

    // State cho Table
    in_table: bool,
    in_table_head: bool,
    header_start_idx: usize,
    header_buffer: String,
}

impl ChunkerContext {
    fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            chunks: Vec::new(),
            current_block: String::new(),
            in_code_block: false,
            in_table: false,
            in_table_head: false,
            header_start_idx: 0,
            header_buffer: String::new(),
        }
    }

    fn handle_table_start(&mut self) {
        self.in_table = true;
        self.header_buffer.clear();
        if !self.current_block.is_empty() && !self.current_block.ends_with('\n') {
            self.current_block.push('\n');
        }
    }

    fn handle_table_end(&mut self) {
        self.in_table = false;
        self.header_buffer.clear();
        self.check_and_flush_block();
    }

    fn handle_table_row_end(&mut self) {
        self.current_block.push('\n');
        // Tiêm Header nếu tràn chunk
        if self.in_table && !self.in_table_head && self.current_block.len() > self.max_bytes {
            self.chunks.push(self.current_block.trim().to_string());
            self.current_block.clear();
            self.current_block.push_str(&self.header_buffer);
        }
    }

    fn handle_code_block_start(&mut self) {
        self.in_code_block = true;
        self.current_block.push_str("```\n");
    }

    fn handle_code_block_end(&mut self) {
        self.in_code_block = false;
        if !self.current_block.ends_with('\n') {
            self.current_block.push('\n');
        }
        self.current_block.push_str("```");

        let hard_limit = self.max_bytes.max(8192);
        if self.current_block.len() > hard_limit {
            self.chunks
                .extend(byte_slice_fallback(&self.current_block, self.max_bytes));
        } else {
            self.chunks.push(self.current_block.clone());
        }
        self.current_block.clear();
    }

    fn check_and_flush_block(&mut self) {
        if self.current_block.len() > self.max_bytes {
            self.chunks.extend(semantic_chunk_paragraph(
                &self.current_block,
                self.max_bytes,
            ));
            self.current_block.clear();
        } else if !self.current_block.trim().is_empty() {
            self.chunks.push(self.current_block.trim().to_string());
            self.current_block.clear();
        }
    }
}

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
    let mut ctx = ChunkerContext::new(max_bytes);

    // Vòng lặp Switchboard (Trạm điều phối)
    for event in parser {
        match event {
            Event::Start(Tag::Table(_)) => ctx.handle_table_start(),
            Event::End(TagEnd::Table) => ctx.handle_table_end(),
            Event::Start(Tag::TableHead) => {
                ctx.in_table_head = true;
                ctx.header_start_idx = ctx.current_block.len();
            }
            Event::End(TagEnd::TableHead) => {
                ctx.in_table_head = false;
                ctx.current_block.push('\n');
                if ctx.header_start_idx <= ctx.current_block.len() {
                    ctx.header_buffer = ctx.current_block[ctx.header_start_idx..].to_string();
                }
            }
            Event::Start(Tag::TableCell) => ctx.current_block.push_str("| "),
            Event::End(TagEnd::TableCell) => ctx.current_block.push(' '),
            Event::End(TagEnd::TableRow) => ctx.handle_table_row_end(),

            Event::Start(Tag::CodeBlock(_)) => ctx.handle_code_block_start(),
            Event::End(TagEnd::CodeBlock) => ctx.handle_code_block_end(),

            Event::Text(t) | Event::Code(t) => ctx.current_block.push_str(&t),
            Event::SoftBreak | Event::HardBreak => ctx.current_block.push('\n'),

            Event::End(TagEnd::Paragraph | TagEnd::Item) => {
                if !ctx.in_code_block && !ctx.in_table {
                    ctx.check_and_flush_block();
                }
            }
            _ => {}
        }
    }

    if !ctx.current_block.trim().is_empty() {
        ctx.check_and_flush_block();
    }

    ctx.chunks
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

    #[test]
    fn test_stress_performance_and_utf8_10mb() {
        use std::time::Instant;

        // Chuỗi mẫu: Chữ Hán, Ả Rập, tiếng Việt, Emoji (đa dạng Byte Size)
        let sample_pattern = "你好! Rust siêu tốc 🚀 مرحبا thế giới. \n";

        // Lặp lại để tạo file ảo 10MB
        // 1 string này khoảng 50 bytes. Lặp 200,000 lần = 10,000,000 bytes (10MB)
        let giant_markdown = sample_pattern.repeat(200_000);

        println!("Nạp xong file 10MB vào RAM. Bắt đầu ép xung...");

        let start_time = Instant::now();
        // Ép chunk nhỏ (500 bytes) để các hàm Fallback/Regex phải làm việc liên tục
        let chunks = ultimate_markdown_chunker(&giant_markdown, 500);
        let duration = start_time.elapsed();

        println!(
            "⏱️ Cắt 10MB thành {} chunks trong {:?}",
            chunks.len(),
            duration
        );

        // Đảm bảo không bị Panic vì cắt trúng giữa ký tự UTF-8
        assert!(!chunks.is_empty());

        // Assert tốc độ: Ở chế độ release, 10MB mất khoảng 0.05 giây.
        // Đặt mốc 2 giây làm chuẩn phòng hờ máy yếu hoặc chạy ở chế độ Debug CI.
        assert!(duration.as_secs_f64() < 2.0, "Thuật toán chạy quá chậm!");
    }
}
