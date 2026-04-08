# llm-wiki

Local-first Personal Knowledge Management (PKM) system theo kiến trúc Event-Driven RAG, expose qua MCP (Model Context Protocol) trên `stdio` để IDE agent (Copilot/Cursor) gọi tool tìm kiếm.

## 1. Mục tiêu dự án

- Watch thư mục markdown local (`data/raw/`)
- Chunk nội dung và tạo embedding qua HTTP API (`reqwest`)
- Upsert/search vector trong Qdrant (`qdrant-client` gRPC)
- Phục vụ tool MCP `search_wiki` qua JSON-RPC 2.0 trên stdin/stdout
- Dùng semantic cache (quantization + LSH + lexical fallback) để giảm round-trip xuống Qdrant

## 2. Tech Stack

- Language: Rust (edition 2021, tương thích stable toolchain)
- Async runtime: `tokio` (full)
- Protocol: JSON-RPC 2.0 over stdio
- Serialization: `serde`, `serde_json`, `serde_yaml`
- File watcher: `notify`
- Vector DB: `qdrant-client`
- HTTP client: `reqwest` (`rustls-tls`)
- Error handling: `anyhow`
- Caching utils: `rustc-hash`

## 3. Kiến trúc module

```text
src/
 main.rs                # Boot toàn bộ task: watcher + MCP server
 config.rs              # Load config.yaml (auto-create + fallback default)
 mcp/
  protocol.rs          # JSON-RPC models
  server.rs            # MCP stdio server + tool routing
 pipeline/
  watcher.rs           # notify + debounce + micro-batch + retry read
  chunker.rs           # chunk markdown
  embedder.rs          # gọi embedding API
 db/
  qdrant.rs            # bulk upsert + search top-k
 cache/
  semantic.rs          # LRU semantic cache + quantization + trigram Jaccard
  lsh.rs               # random-projection LSH
scripts/
 pre-commit.sh          # fmt + clippy(strict) + test
 setup-security.sh      # harden data/raw + install hook local
```

## 4. Prerequisites

Yêu cầu tối thiểu:

1. Rust stable qua `rustup`
2. Qdrant chạy local hoặc remote
3. Embedding endpoint tương thích payload JSON (ví dụ Ollama/LiteLLM/vLLM gateway)

Ví dụ cài Rust:

```bash
curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
source "$HOME/.cargo/env"
```

## 5. Development bootstrap

Mục này dành cho dev setup máy mới từ đầu (fresh machine).

### 5.1 Tool bắt buộc

1. Git
2. Rust stable qua `rustup`
3. C toolchain cơ bản cho OS hiện tại

### 5.2 Tool theo use-case

1. Cross-build Windows từ Linux: `mingw-w64`
2. Linux static build từ Windows: target `x86_64-unknown-linux-musl`
3. Qdrant runtime (local hoặc remote)
4. Embedding backend (Ollama / LiteLLM / vLLM gateway)

### 5.3 Cài nhanh theo OS

Linux (Ubuntu/Debian):

```bash
sudo apt update
sudo apt install -y build-essential git curl pkg-config
curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
source "$HOME/.cargo/env"
```

macOS (Homebrew):

```bash
brew install git rustup-init
rustup-init -y --default-toolchain stable
source "$HOME/.cargo/env"
```

Windows (PowerShell + winget):

```powershell
winget install --id Git.Git -e
winget install --id Rustlang.Rustup -e
```

### 5.4 Verify môi trường

```bash
rustup --version
rustc --version
cargo --version
cargo check
```

### 5.5 Optional nhưng khuyến nghị

```bash
rustup component add rustfmt clippy
./scripts/setup-security.sh
```

## 6. Setup nhanh

```bash
git clone <repo-url>
cd llm-wiki

# Cài khiên bảo vệ local (read-only data/raw + pre-commit hook)
./scripts/setup-security.sh

# Build/check
cargo check
```

## 7. Cấu hình `config.yaml`

Ứng dụng load file bằng:

- `AppConfig::load_or_create("config.yaml")`

Behavior:

1. Nếu `config.yaml` chưa tồn tại: tự tạo template từ `Default`, log warning ra `stderr`, tiếp tục chạy.
2. Nếu parse lỗi: fallback `Default`, log warning ra `stderr`.
3. Có thể override từng phần (partial override), không cần khai báo đủ tất cả field.

Mẫu cấu hình:

```yaml
raw_data_path: data/raw
qdrant_url: http://127.0.0.1:6334
qdrant_collection: llm_wiki_chunks

embedding:
 base_url: http://127.0.0.1:11434
 endpoint: /api/embeddings
 model: nomic-embed-text
 timeout_secs: 30
```

Ví dụ chỉ override `base_url` để cắm host khác:

```yaml
embedding:
 base_url: http://192.168.1.10:11434
```

## 8. Security và Integrity

### 7.1 Read-only data source

Script [scripts/setup-security.sh](scripts/setup-security.sh) sẽ:

1. Tạo `data/raw/`
2. Áp quyền read-only cho `data/raw/` trên Linux/macOS (`chmod -R a-w`)

### 7.2 Pre-commit gate

Hook local sẽ gọi [scripts/pre-commit.sh](scripts/pre-commit.sh), gồm:

1. `cargo fmt -- --check`
2. `cargo clippy -- -D warnings -W clippy::pedantic -W clippy::await_holding_lock -W clippy::unwrap_used`
3. `cargo test`

Script có fallback tìm `cargo` ở `~/.cargo/bin/cargo` nếu PATH tối giản.

## 9. Ingestion Pipeline

Luồng ingest trong [src/pipeline/watcher.rs](src/pipeline/watcher.rs):

1. `notify` producer nhận event `Create`/`Modify` của file `.md`
2. Đẩy `PathBuf` vào `tokio::sync::mpsc`
3. Consumer debounce 2s bằng `tokio::select!` + `HashSet<PathBuf>` để deduplicate
4. Khi timer hết hạn: drain batch, đọc file, chunk, embed, bulk upsert lên Qdrant

Hardening khi đọc file:

- Retry read nhiều lần khi gặp lỗi retryable (`WouldBlock`, `Interrupted`, `TimedOut`, `PermissionDenied`)
- Kiểm tra kích thước file ổn định (2 lần metadata cách nhau 100ms) trước khi đọc
- Log warning ra `stderr`

## 10. MCP Server

Luồng MCP trong [src/mcp/server.rs](src/mcp/server.rs):

1. Giao tiếp JSON-RPC 2.0 qua `stdin/stdout`
2. Method hỗ trợ:

   - `initialize`
   - `tools/list`
   - `tools/call`

3. Tool hiện có: `search_wiki`

- Input schema: `{ query: string }`
- Search path: semantic cache -> (miss) embed query -> Qdrant top 5

Lưu ý quan trọng:

- Không dùng `println!` để log debug trong runtime MCP
- Chỉ log qua `stderr` (`eprintln!`) để không làm hỏng protocol trên stdout

## 11. Chạy dự án

```bash
cargo run
```

App sẽ:

1. Load/tạo `config.yaml`
2. Khởi tạo Qdrant client + embedding client
3. Spawn watcher task
4. Spawn MCP server task

## 12. Build cross-platform

### 11.1 Build dựa trên môi trường development

Sẽ build ra file `.exe` hoặc ELF tuỳ vào môi trường dev:

```bash
cargo build --release
# File app sẽ nằm ở: target/release
```

Muốn build ra file Linux từ máy Windows bằng MUSL để giảm phụ thuộc thư viện hệ điều hành:

```bash
# 1. Tải target linux về máy
rustup target add x86_64-unknown-linux-musl

# 2. Build ép sang chuẩn Linux
cargo build --target x86_64-unknown-linux-musl --release
# File app sẽ nằm ở: target/x86_64-unknown-linux-musl/release/llm-wiki
```

### 11.2 Build trên Linux/macOS

Muốn build ra file `.exe` cho máy Windows:

```bash
# 1. Tải target windows
rustup target add x86_64-pc-windows-gnu

# 2. Cài thêm bộ biên dịch C cho Windows (trên Ubuntu)
sudo apt install mingw-w64

# 3. Build ép sang Windows
cargo build --target x86_64-pc-windows-gnu --release
# File sẽ là: target/x86_64-pc-windows-gnu/release/llm-wiki.exe
```

## 13. Development commands

```bash
# Format
cargo fmt

# Lint strict
cargo clippy -- -D warnings -W clippy::pedantic -W clippy::await_holding_lock -W clippy::unwrap_used

# Test
cargo test
```

## 14. Troubleshooting

### `cargo: command not found` khi commit

- Chạy lại: `./scripts/setup-security.sh`
- Kiểm tra `~/.cargo/bin/cargo` tồn tại

### `config.yaml` không tồn tại

- Đây là behavior bình thường, app sẽ tự tạo template và dùng default.

### Không search được từ Qdrant

- Kiểm tra `qdrant_url`, collection name, network connectivity
- Kiểm tra embedding endpoint trả đúng JSON có field `embedding: [f32, ...]`

## 15. Trạng thái hiện tại

- Đã có khung đầy đủ watcher + MCP + cache + qdrant integration
- Đã có security bootstrap script và pre-commit gate
- Sẵn sàng để mở rộng thêm: schema migration cho Qdrant collection, metrics, observability, integration tests end-to-end
