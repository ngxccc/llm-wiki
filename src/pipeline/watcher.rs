use crate::db::qdrant::{ChunkVector, QdrantStore};
use crate::pipeline::chunker::ultimate_markdown_chunker;
use crate::pipeline::embedder::EmbeddingClient;
use anyhow::{Context, Result};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const DEBOUNCE_WINDOW: Duration = Duration::from_secs(2);
const READ_RETRY_DELAY: Duration = Duration::from_millis(150);
const SIZE_STABILITY_DELAY: Duration = Duration::from_millis(100);
const MAX_READ_RETRIES: usize = 5;
const RAM_BUFFER_LIMIT: usize = 5 * 1024 * 1024;

pub async fn run_watcher(
    raw_dir: PathBuf,
    embedder: EmbeddingClient,
    qdrant: QdrantStore,
    cancel_token: CancellationToken,
    vector_dimension: usize,
) -> Result<()> {
    let (event_tx, mut event_rx) = mpsc::channel::<PathBuf>(1024);

    eprintln!(
        "watcher started: watching markdown under {}",
        raw_dir.display()
    );

    let mut watcher = RecommendedWatcher::new(
        {
            let event_tx = event_tx.clone();
            move |result: notify::Result<Event>| {
                if let Ok(event) = result {
                    if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                        for path in event.paths {
                            if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
                                let _ = event_tx.blocking_send(path);
                            }
                        }
                    }
                }
            }
        },
        Config::default(),
    )
    .context("failed to initialize file watcher")?;

    watcher
        .watch(Path::new(&raw_dir), RecursiveMode::Recursive)
        .context("failed to watch raw data path")?;

    let initial_paths = collect_markdown_files(&raw_dir);
    if !initial_paths.is_empty() {
        eprintln!(
            "watcher initial bootstrap: ingesting {} existing markdown file(s)",
            initial_paths.len()
        );
        process_batch(&initial_paths, &embedder, &qdrant, vector_dimension).await?;
    }

    run_batch_consumer(
        &mut event_rx,
        embedder,
        qdrant,
        cancel_token,
        vector_dimension,
    )
    .await
}

/// Event micro-batching consumer.
///
/// Time: O(e) ingest where e is incoming events; dedup keeps per-window processing unique.
/// Space: O(u) where u is unique changed files in the active debounce window.
async fn run_batch_consumer(
    event_rx: &mut mpsc::Receiver<PathBuf>,
    embedder: EmbeddingClient,
    qdrant: QdrantStore,
    cancel_token: CancellationToken,
    vector_dimension: usize,
) -> Result<()> {
    let mut pending = HashSet::<PathBuf>::new();
    let timer = tokio::time::sleep(Duration::from_secs(24 * 60 * 60));
    tokio::pin!(timer);

    loop {
        tokio::select! {
            () = cancel_token.cancelled() => {
                eprintln!("Watcher received shutdown signal.");
                if !pending.is_empty() {
                    let batch = pending.drain().collect::<Vec<_>>();
                    eprintln!("Flushing {} pending file(s) before shutdown.", batch.len());
                    process_batch(&batch, &embedder, &qdrant, vector_dimension).await?;
                }
                eprintln!("Watcher shutdown completed.");
                break;
            }
            maybe_path = event_rx.recv() => {
                let Some(path) = maybe_path else {
                    break;
                };

                eprintln!("watcher queued change: {}", path.display());
                pending.insert(path);
                timer.as_mut().reset(Instant::now() + DEBOUNCE_WINDOW);
            }
            () = &mut timer, if !pending.is_empty() => {
                let batch = pending.drain().collect::<Vec<_>>();
                process_batch(&batch, &embedder, &qdrant, vector_dimension).await?;
                timer.as_mut().reset(Instant::now() + Duration::from_secs(24 * 60 * 60));
            }
        }
    }

    if !pending.is_empty() {
        let batch = pending.drain().collect::<Vec<_>>();
        process_batch(&batch, &embedder, &qdrant, vector_dimension).await?;
    }

    Ok(())
}

async fn process_batch(
    paths: &[PathBuf],
    embedder: &EmbeddingClient,
    qdrant: &QdrantStore,
    vector_dimension: usize,
) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }

    eprintln!("watcher processing batch with {} file(s)", paths.len());

    'file_loop: for path in paths {
        // Đảm bảo file ổn định trước khi đọc
        if let Err(_e) = wait_for_stable_file(path).await {
            continue;
        }

        let initial_meta = tokio::fs::metadata(path)
            .await
            .with_context(|| format!("failed to read metadata for {}", path.display()))?;
        let initial_mtime = initial_meta
            .modified()
            .with_context(|| format!("failed to read modified time for {}", path.display()))?;
        let initial_size = initial_meta.len();

        let mut file = tokio::fs::File::open(path)
            .await
            .with_context(|| format!("failed to open file {}", path.display()))?;
        let reader = BufReader::new(&mut file);

        // GATHER GLOBAL CONTEXT (Link Extraction)
        eprintln!("Pass 1: Scanning global context for {}...", path.display());
        let mut global_links = String::new();
        let mut lines = reader.lines();
        let link_re = ref_link_regex();

        while let Ok(Some(line)) = lines.next_line().await {
            // Nếu dòng này là khai báo link, cất nó vào "túi khôn"
            if link_re.is_match(&line) {
                global_links.push_str(&line);
                global_links.push('\n');
            }
        }

        // AST PARSING & CHUNKING
        eprintln!("Pass 2: Chunking & Embedding 5MB streams...");
        // Tua ngược con trỏ file về lại vị trí byte 0 (Vạch xuất phát)
        file.rewind()
            .await
            .with_context(|| format!("failed to rewind file {}", path.display()))?;

        // Reset lại reader cho file vừa tua
        let reader = BufReader::new(&mut file);
        let mut lines = reader.lines();

        let mut string_buffer = String::with_capacity(RAM_BUFFER_LIMIT);
        let mut in_code_block = false;
        let mut vectors = Vec::new();
        let mut global_chunk_index = 0;

        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim_start().starts_with("```") {
                in_code_block = !in_code_block;
            }

            string_buffer.push_str(&line);
            string_buffer.push('\n');

            if string_buffer.len() >= RAM_BUFFER_LIMIT && !in_code_block {
                // 💉 INJECTION MAGIC: Nhồi đống Link gom được ở Pass 1 vào đầu Buffer 5MB!
                // Nhờ vậy, AST sẽ tự động resolve được [my_link] dù nó nằm ở cuối file.
                let chunk_payload = format!("{global_links}\n\n{string_buffer}");

                global_chunk_index = process_and_flush_buffer(
                    &chunk_payload,
                    path,
                    embedder,
                    qdrant,
                    vector_dimension,
                    &mut vectors,
                    global_chunk_index,
                )
                .await?;

                string_buffer.clear();
            }
        }

        // Xử lý nốt phần đuôi file
        if !string_buffer.trim().is_empty() {
            let chunk_payload = format!("{global_links}\n\n{string_buffer}");
            process_and_flush_buffer(
                &chunk_payload,
                path,
                embedder,
                qdrant,
                vector_dimension,
                &mut vectors,
                global_chunk_index,
            )
            .await?;
        }

        if let Ok(final_meta) = tokio::fs::metadata(path).await {
            let final_mtime = final_meta.modified().unwrap_or(initial_mtime);
            let final_size = final_meta.len();

            // Nếu file bị user sửa đổi (hoặc size đổi) trong lúc ta đang chạy 2 Pass
            if final_mtime != initial_mtime || final_size != initial_size {
                eprintln!(
                    "🚨 TOCTOU Detected! File {} was modified during processing.",
                    path.display()
                );
                eprintln!("🗑️ Aborting current state. The Watcher will re-process the new event automatically.");

                // Trả về lỗi để thoát lô xử lý này. Watcher sẽ tự kích hoạt lại lô mới!
                continue 'file_loop;
            }
        }

        eprintln!(
            "✅ File {} processed atomically with OCC success!",
            path.display()
        );
    }

    Ok(())
}

async fn process_and_flush_buffer(
    text_buffer: &str,
    path: &Path,
    embedder: &EmbeddingClient,
    qdrant: &QdrantStore,
    vector_dimension: usize,
    vectors: &mut Vec<ChunkVector>,
    mut chunk_index: usize,
) -> Result<usize> {
    // 1. Quăng 5MB text vào Chunker xịn xò của mình
    let chunks = ultimate_markdown_chunker(text_buffer, 800);
    if chunks.is_empty() {
        return Ok(chunk_index);
    }

    let chunk_refs = chunks.iter().map(String::as_str).collect::<Vec<_>>();
    let mut embeddings = Vec::with_capacity(chunk_refs.len());

    // 2. Gọi API Embeddings (Đã chia lô batch bên trong)
    for chunk_batch in chunk_refs.chunks(embedder.max_batch_size()) {
        let mut batch_embeddings = embedder
            .embed_batch_with_retry(chunk_batch, 3)
            .await
            .unwrap_or_else(|e| {
                eprintln!("Batch embed failed: {e}");
                vec![vec![0.0; vector_dimension]; chunk_batch.len()] // Fallback rác nếu fail
            });
        embeddings.append(&mut batch_embeddings);
    }

    // 3. Đóng gói thành Vector Point
    for (chunk, embedding) in chunks.into_iter().zip(embeddings) {
        let final_embedding = if embedding.len() == vector_dimension {
            embedding
        } else {
            embed_chunk(&chunk, vector_dimension)
        };

        vectors.push(ChunkVector {
            source_path: path.display().to_string(),
            chunk_index,
            text: chunk,
            embedding: final_embedding,
        });
        chunk_index += 1;
    }

    // 4. Bơm mẹ nó lên Qdrant luôn cho nóng (Giữ mảng vectors nhỏ)
    if !vectors.is_empty() {
        qdrant.bulk_upsert(vectors).await?;
        eprintln!("Stream flushed {} vectors to Qdrant...", vectors.len());
        vectors.clear(); // XẢ RAM TẦNG 2!
    }

    Ok(chunk_index) // Trả về index để đếm tiếp cho mẻ sau
}

async fn wait_for_stable_file(path: &Path) -> Result<()> {
    for _ in 0..MAX_READ_RETRIES {
        if has_stable_size(path).await? {
            return Ok(());
        }
        tokio::time::sleep(READ_RETRY_DELAY).await;
    }
    anyhow::bail!("File is constantly modifying, skipped.")
}

fn embed_chunk(text: &str, dimension: usize) -> Vec<f32> {
    let mut vector = vec![0.0_f32; dimension];
    for (index, byte) in text.bytes().enumerate() {
        let bucket = index % dimension;
        let signed = f32::from(byte) / 255.0;
        vector[bucket] += signed;
    }

    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }

    vector
}

fn collect_markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut markdown_files = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(current_dir) = stack.pop() {
        let entries = match fs::read_dir(&current_dir) {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!(
                    "warning: failed to list directory {}: {error}",
                    current_dir.display()
                );
                continue;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    eprintln!(
                        "warning: failed to read dir entry under {}: {error}",
                        current_dir.display()
                    );
                    continue;
                }
            };

            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }

            if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
                markdown_files.push(path);
            }
        }
    }

    markdown_files
}

async fn has_stable_size(path: &Path) -> io::Result<bool> {
    let first = tokio::fs::metadata(path).await?;
    let first_len = first.len();
    tokio::time::sleep(SIZE_STABILITY_DELAY).await;
    let second = tokio::fs::metadata(path).await?;
    Ok(first_len == second.len())
}

fn ref_link_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?m)^\[([^\]]+)\]:\s*(.+)$")
            .expect("internal error: ref_link_regex pattern must be valid")
    })
}
