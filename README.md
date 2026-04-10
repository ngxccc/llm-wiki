# LLM Wiki: The Event-Driven Local RAG & MCP Server

[![Written in Rust](https://img.shields.io/badge/Written_in-Rust-orange.svg)](https://www.rust-lang.org/)
[![Protocol](https://img.shields.io/badge/Protocol-MCP-blue.svg)](https://modelcontextprotocol.io/)
[![Vector DB](https://img.shields.io/badge/Vector_DB-Qdrant-purple.svg)](https://qdrant.tech/)

Local-first Personal Knowledge Management (PKM) system theo kiến trúc Event-Driven RAG, expose qua MCP (Model Context Protocol) trên `stdio` để IDE agent (Copilot/Cursor) gọi tool tìm kiếm.

## Table of Contents

- [LLM Wiki: The Event-Driven Local RAG \& MCP Server](#llm-wiki-the-event-driven-local-rag--mcp-server)
  - [Table of Contents](#table-of-contents)
  - [1. Mục tiêu dự án](#1-mục-tiêu-dự-án)
  - [2. Tech Stack](#2-tech-stack)
  - [3. Kiến trúc module](#3-kiến-trúc-module)
  - [4. Prerequisites](#4-prerequisites)
  - [5. Development bootstrap](#5-development-bootstrap)
    - [5.1 Tool bắt buộc](#51-tool-bắt-buộc)
    - [5.2 Tool theo use-case](#52-tool-theo-use-case)
    - [5.3 Cài nhanh theo OS](#53-cài-nhanh-theo-os)
    - [5.4 Verify môi trường](#54-verify-môi-trường)
    - [5.5 Optional nhưng khuyến nghị](#55-optional-nhưng-khuyến-nghị)
    - [5.6 Setup LiteLLM gateway](#56-setup-litellm-gateway)
    - [5.7 VS Code MCP setup](#57-vs-code-mcp-setup)
  - [6. Setup nhanh](#6-setup-nhanh)
  - [7. Cấu hình `config.yaml`](#7-cấu-hình-configyaml)
    - [7.1 Config reference (app)](#71-config-reference-app)
    - [7.2 Config reference (Qdrant server)](#72-config-reference-qdrant-server)
  - [8. Dùng thực tế để test MCP](#8-dùng-thực-tế-để-test-mcp)
  - [9. Security và Integrity](#9-security-và-integrity)
    - [9.1 Read-only data source](#91-read-only-data-source)
    - [9.2 Pre-commit gate](#92-pre-commit-gate)
  - [10. Ingestion Pipeline](#10-ingestion-pipeline)
  - [11. MCP Server](#11-mcp-server)
  - [12. Chạy dự án](#12-chạy-dự-án)
  - [13. Build cross-platform](#13-build-cross-platform)
    - [13.1 Build dựa trên môi trường development](#131-build-dựa-trên-môi-trường-development)
    - [13.2 Build trên Linux/macOS](#132-build-trên-linuxmacos)
  - [14. Development commands](#14-development-commands)
  - [15. Troubleshooting](#15-troubleshooting)
    - [`cargo: command not found` khi commit](#cargo-command-not-found-khi-commit)
    - [`config.yaml` không tồn tại](#configyaml-không-tồn-tại)
    - [Không search được từ Qdrant](#không-search-được-từ-qdrant)

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
3. LiteLLM gateway cho embedding API (OpenAI-compatible `/v1/embeddings`)

Nếu bạn muốn chạy Qdrant local bằng Docker để tránh crash khi app khởi động, dùng:

```bash
docker run -d -p 6333:6333 -p 6334:6334 -v $(pwd)/qdrant_data:/qdrant/storage qdrant/qdrant
```

Lệnh này sẽ expose Qdrant HTTP/gRPC và persist data vào thư mục `qdrant_data/` trong workspace hiện tại.

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
4. LiteLLM gateway + provider embedding phía sau (Ollama/OpenAI/Cohere/...)

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

# Override vị trí lưu data/raw nếu cần
export LLM_WIKI_RAW_DATA_PATH=/path/to/your/raw-data
```

### 5.6 Setup LiteLLM gateway

Project hiện mặc định gọi embedding qua LiteLLM ở:

- Base URL: `http://127.0.0.1:4000`
- Endpoint: `/v1/embeddings`

Bạn có thể chạy LiteLLM local bằng Docker:

```bash
docker run --rm -p 4000:4000 \
  -e LITELLM_MASTER_KEY=sk-local-dev \
  -e OPENAI_API_KEY=$OPENAI_API_KEY \
  ghcr.io/berriai/litellm:main-latest \
  --model openai/text-embedding-3-small
```

Hoặc nếu bạn dùng Ollama local, map model qua LiteLLM config để vẫn giữ chuẩn `/v1/embeddings` cho app.

Nếu LiteLLM yêu cầu auth, set trong `config.yaml`:

```yaml
embedding:
  api_key: sk-local-dev
```

### 5.7 VS Code MCP setup

Workspace đã có file cấu hình MCP ở [.vscode/mcp.json](.vscode/mcp.json) để Copilot/Cursor chạy server qua stdio.

Nó trỏ vào `cargo run --quiet` và mặc định expose `data/raw/` từ gốc repository. Nếu bạn muốn test nhanh ngay sau khi clone:

1. Chạy Qdrant local bằng Docker.
2. Chạy `./scripts/setup-security.sh` để tạo `data/raw/` và cài hook.
3. Mở project trong VS Code rồi bật MCP server từ cấu hình workspace.

## 6. Setup nhanh

```bash
git clone <repo-url>
cd llm-wiki

# Start LiteLLM (cần chạy trước khi app ingest/search)
docker run --rm -p 4000:4000 \
  -e LITELLM_MASTER_KEY=sk-local-dev \
  -e OPENAI_API_KEY=$OPENAI_API_KEY \
  ghcr.io/berriai/litellm:main-latest \
  --model openai/text-embedding-3-small

# Cài khiên bảo vệ local (read-only data/raw + pre-commit hook)
./scripts/setup-security.sh

# Build/check
cargo check
```

## 7. Cấu hình `config.yaml`

Ứng dụng load file bằng:

- `AppConfig::load_or_create("config.yaml")`

Mặc định app sẽ đọc và ghi dữ liệu ở `data/raw/` ngay tại gốc project. Nếu bạn chạy file build từ thư mục gốc repository thì nó vẫn dùng đúng path này. Muốn đổi vị trí lưu `data/raw`, set biến môi trường `LLM_WIKI_RAW_DATA_PATH`; nếu không set, app sẽ fallback về `data/raw/`.

Behavior:

1. Nếu `config.yaml` chưa tồn tại: tự tạo template từ `Default`, log warning ra `stderr`, tiếp tục chạy.
2. Nếu parse lỗi: fallback `Default`, log warning ra `stderr`.
3. Có thể override từng phần (partial override), không cần khai báo đủ tất cả field.

Mẫu cấu hình: [config.example.yaml](config.example.yaml)

```bash
cp config.example.yaml config.yaml
```

Khuyến nghị production: dùng LiteLLM làm gateway chuẩn cho embedding để app luôn gọi một interface OpenAI-style (`/v1/embeddings`, payload `input` dạng mảng), còn việc map sang provider cụ thể (Ollama/Cohere/OpenAI) do LiteLLM xử lý.

Ví dụ chỉ override `base_url` để cắm host khác:

```yaml
embedding:
 base_url: http://192.168.1.10:4000
```

### 7.1 Config reference (app)

- `raw_data_path`
  - Mục đích: thư mục markdown nguồn để watcher ingest.
  - Default: `data/raw`
  - Override bằng env: `LLM_WIKI_RAW_DATA_PATH`

- `qdrant_url`
  - Mục đích: URL để app kết nối Qdrant.
  - Default: `http://127.0.0.1:6334`
  - Ghi chú: app hiện dùng gRPC port của Qdrant.

- `qdrant_collection`
  - Mục đích: tên collection chứa vectors/chunks.
  - Default: `llm_wiki_chunks`

- `embedding.base_url`
  - Mục đích: host LiteLLM gateway.
  - Default: `http://127.0.0.1:4000`

- `embedding.endpoint`
  - Mục đích: path API embedding OpenAI-compatible trên LiteLLM.
  - Default: `/v1/embeddings`

- `embedding.model`
  - Mục đích: model embedding sử dụng khi gọi API.
  - Default trong app: `bge-m3`

- `embedding.timeout_secs`
  - Mục đích: timeout cho mỗi HTTP request embedding.

- `embedding.api_key`
  - Mục đích: Bearer token gửi cho gateway/provider nếu endpoint yêu cầu auth.

- `embedding.dimensions`
  - Mục đích: yêu cầu giảm chiều vector (chỉ có hiệu lực khi model/gateway hỗ trợ).
  - Default: `1024` (fallback nếu không set)
  - Ghi chú: giá trị này được truyền động vào watcher thay vì hardcoded.

- `embedding.max_batch_size`
  - Mục đích: giới hạn số chunk mỗi request để tránh payload quá lớn.

- `qdrant_api_key` (optional)
  - Mục đích: Bearer token để authenticate với Qdrant nếu server yêu cầu.

### 7.2 Config reference (Qdrant server)

Nếu bạn chạy Qdrant binary thay vì Docker, dùng file mẫu [qdrant-server.example.yaml](qdrant-server.example.yaml):

```bash
cp qdrant-server.example.yaml qdrant-server.yaml
qdrant --config-path ./qdrant-server.yaml
```

Giải thích nhanh:

- `storage.storage_path`: nơi Qdrant lưu dữ liệu local.
- `service.http_port`: cổng HTTP (mặc định app nên trỏ vào cổng này).
- `service.grpc_port`: cổng gRPC cho các client cần gRPC trực tiếp.

## 8. Dùng thực tế để test MCP

Trong repo đã có sample data ở [data/raw/](data/raw/) để bạn test ngay.

Luồng test nhanh:

1. Start Qdrant local bằng Docker.
2. Start LiteLLM gateway ở `http://127.0.0.1:4000`.
3. Chạy `cargo run` ở root project hoặc để VS Code gọi server qua [.vscode/mcp.json](.vscode/mcp.json).
4. Dùng tool `search_wiki` với query như:
   - `Where is the raw markdown data stored?`
   - `How do I start the MCP server?`
   - `What command runs Qdrant locally?`

Các file sample hiện có:

- [data/raw/overview.md](data/raw/overview.md)
- [data/raw/mcp-usage.md](data/raw/mcp-usage.md)
- [data/raw/rust-notes.md](data/raw/rust-notes.md)

## 9. Security và Integrity

### 9.1 Read-only data source

Script [scripts/setup-security.sh](scripts/setup-security.sh) sẽ:

1. Tạo `data/raw/`
2. Áp quyền read-only cho `data/raw/` trên Linux/macOS (`chmod -R a-w`)

### 9.2 Pre-commit gate

Hook local sẽ gọi [scripts/pre-commit.sh](scripts/pre-commit.sh), gồm:

1. `cargo fmt -- --check`
2. `cargo clippy -- -D warnings -W clippy::pedantic -W clippy::await_holding_lock -W clippy::unwrap_used`
3. `cargo test`

Script có fallback tìm `cargo` ở `~/.cargo/bin/cargo` nếu PATH tối giản.

## 10. Ingestion Pipeline

Luồng ingest trong [src/pipeline/watcher.rs](src/pipeline/watcher.rs):

1. `notify` producer nhận event `Create`/`Modify` của file `.md`
2. Đẩy `PathBuf` vào `tokio::sync::mpsc`
3. Consumer debounce 2s bằng `tokio::select!` + `HashSet<PathBuf>` để deduplicate
4. Khi timer hết hạn: drain batch, đọc file, chunk, embed, bulk upsert lên Qdrant

Hardening khi đọc file:

- Retry read nhiều lần khi gặp lỗi retryable (`WouldBlock`, `Interrupted`, `TimedOut`, `PermissionDenied`)
- Kiểm tra kích thước file ổn định (2 lần metadata cách nhau 100ms) trước khi đọc
- Log warning ra `stderr`

## 11. MCP Server

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

## 12. Chạy dự án

```bash
cargo run
```

App sẽ:

1. Load/tạo `config.yaml`
2. Khởi tạo Qdrant client + embedding client
3. Spawn watcher task
4. Spawn MCP server task

## 13. Build cross-platform

### 13.1 Build dựa trên môi trường development

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

### 13.2 Build trên Linux/macOS

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

## 14. Development commands

```bash
# Format
cargo fmt

# Lint strict
cargo clippy -- -D warnings -W clippy::pedantic -W clippy::await_holding_lock -W clippy::unwrap_used

# Test
cargo test
```

## 15. Troubleshooting

### `cargo: command not found` khi commit

- Chạy lại: `./scripts/setup-security.sh`
- Kiểm tra `~/.cargo/bin/cargo` tồn tại

### `config.yaml` không tồn tại

- Đây là behavior bình thường, app sẽ tự tạo template và dùng default.

### Không search được từ Qdrant

- Kiểm tra `qdrant_url`, collection name, network connectivity
- Kiểm tra embedding endpoint trả đúng JSON có field `embedding: [f32, ...]`
