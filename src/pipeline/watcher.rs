use crate::db::qdrant::{ChunkVector, QdrantStore};
use crate::pipeline::chunker::semantic_chunk;
use crate::pipeline::embedder::EmbeddingClient;
use anyhow::{Context, Result};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const DEBOUNCE_WINDOW: Duration = Duration::from_secs(2);
const READ_RETRY_DELAY: Duration = Duration::from_millis(150);
const SIZE_STABILITY_DELAY: Duration = Duration::from_millis(100);
const MAX_READ_RETRIES: usize = 5;

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
    let mut vectors = Vec::new();

    'file_loop: for path in paths {
        let content = match read_markdown_with_retry(path).await {
            Ok(content) => content,
            Err(error) => {
                eprintln!(
                    "watcher skipped unreadable file {}: {error:#}",
                    path.display()
                );
                continue;
            }
        };

        let chunks = semantic_chunk(&content, 800, 1);
        if chunks.is_empty() {
            continue;
        }

        let chunk_refs = chunks.iter().map(String::as_str).collect::<Vec<_>>();
        let mut embeddings = Vec::with_capacity(chunk_refs.len());
        let max_batch_size = embedder.max_batch_size();

        for chunk_batch in chunk_refs.chunks(max_batch_size) {
            let mut batch_embeddings = match embedder.embed_batch_with_retry(chunk_batch, 3).await {
                Ok(embeddings) => embeddings,
                Err(error) => {
                    eprintln!(
                        "watcher skipped file due to embedding batch failure (file: {}): {error:#}",
                        path.display()
                    );
                    continue 'file_loop;
                }
            };
            embeddings.append(&mut batch_embeddings);
        }

        for (index, (chunk, embedding)) in chunks.into_iter().zip(embeddings).enumerate() {
            let embedding = if embedding.len() == vector_dimension {
                embedding
            } else {
                eprintln!(
                    "watcher embedding dimension mismatch (file: {}, chunk: {}, got {}, expected {}). Falling back to deterministic embedding.",
                    path.display(),
                    index,
                    embedding.len(),
                    vector_dimension
                );
                embed_chunk(&chunk, vector_dimension)
            };

            vectors.push(ChunkVector {
                source_path: path.display().to_string(),
                chunk_index: index,
                text: chunk,
                embedding,
            });
        }
    }

    if vectors.is_empty() {
        eprintln!("watcher batch produced no vectors to upsert");
        return Ok(());
    }

    qdrant.bulk_upsert(&vectors).await?;
    eprintln!("watcher upserted {} vector(s) to Qdrant", vectors.len());
    Ok(())
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

async fn read_markdown_with_retry(path: &Path) -> Result<String> {
    let mut last_error: Option<io::Error> = None;

    for attempt in 1..=MAX_READ_RETRIES {
        match has_stable_size(path).await {
            Ok(true) => {}
            Ok(false) => {
                eprintln!(
                    "warning: file {} is still changing (attempt {attempt}/{MAX_READ_RETRIES}), retrying",
                    path.display()
                );
                tokio::time::sleep(READ_RETRY_DELAY).await;
                continue;
            }
            Err(error) if is_retryable_io(error.kind()) => {
                eprintln!(
                    "warning: failed to stat {} ({error}) (attempt {attempt}/{MAX_READ_RETRIES}), retrying",
                    path.display()
                );
                last_error = Some(error);
                tokio::time::sleep(READ_RETRY_DELAY).await;
                continue;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to stat file {}", path.display()));
            }
        }

        match tokio::fs::read_to_string(path).await {
            Ok(content) => return Ok(content),
            Err(error) if is_retryable_io(error.kind()) => {
                eprintln!(
                    "warning: failed to read {} ({error}) (attempt {attempt}/{MAX_READ_RETRIES}), retrying",
                    path.display()
                );
                last_error = Some(error);
                tokio::time::sleep(READ_RETRY_DELAY).await;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to read file {}", path.display()));
            }
        }
    }

    if let Some(error) = last_error {
        return Err(error)
            .with_context(|| format!("exhausted retries while reading {}", path.display()));
    }

    anyhow::bail!("exhausted retries while reading {}", path.display());
}

async fn has_stable_size(path: &Path) -> io::Result<bool> {
    let first = tokio::fs::metadata(path).await?;
    let first_len = first.len();
    tokio::time::sleep(SIZE_STABILITY_DELAY).await;
    let second = tokio::fs::metadata(path).await?;
    Ok(first_len == second.len())
}

fn is_retryable_io(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::WouldBlock
            | io::ErrorKind::Interrupted
            | io::ErrorKind::TimedOut
            | io::ErrorKind::PermissionDenied
    )
}
