use crate::db::qdrant::{ChunkVector, QdrantStore};
use crate::pipeline::chunker::chunk_markdown;
use crate::pipeline::embedder::EmbeddingClient;
use anyhow::{Context, Result};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};

const DEBOUNCE_WINDOW: Duration = Duration::from_secs(2);
const READ_RETRY_DELAY: Duration = Duration::from_millis(150);
const SIZE_STABILITY_DELAY: Duration = Duration::from_millis(100);
const MAX_READ_RETRIES: usize = 5;

pub async fn run_watcher(
    raw_dir: PathBuf,
    embedder: EmbeddingClient,
    qdrant: QdrantStore,
) -> Result<()> {
    let (event_tx, mut event_rx) = mpsc::channel::<PathBuf>(1024);

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

    run_batch_consumer(&mut event_rx, embedder, qdrant).await
}

/// Event micro-batching consumer.
///
/// Time: O(e) ingest where e is incoming events; dedup keeps per-window processing unique.
/// Space: O(u) where u is unique changed files in the active debounce window.
async fn run_batch_consumer(
    event_rx: &mut mpsc::Receiver<PathBuf>,
    embedder: EmbeddingClient,
    qdrant: QdrantStore,
) -> Result<()> {
    let mut pending = HashSet::<PathBuf>::new();
    let timer = tokio::time::sleep(Duration::from_secs(24 * 60 * 60));
    tokio::pin!(timer);

    loop {
        tokio::select! {
            maybe_path = event_rx.recv() => {
                let Some(path) = maybe_path else {
                    break;
                };

                pending.insert(path);
                timer.as_mut().reset(Instant::now() + DEBOUNCE_WINDOW);
            }
            () = &mut timer, if !pending.is_empty() => {
                let batch = pending.drain().collect::<Vec<_>>();
                process_batch(&batch, &embedder, &qdrant).await?;
                timer.as_mut().reset(Instant::now() + Duration::from_secs(24 * 60 * 60));
            }
        }
    }

    if !pending.is_empty() {
        let batch = pending.drain().collect::<Vec<_>>();
        process_batch(&batch, &embedder, &qdrant).await?;
    }

    Ok(())
}

async fn process_batch(
    paths: &[PathBuf],
    embedder: &EmbeddingClient,
    qdrant: &QdrantStore,
) -> Result<()> {
    let mut vectors = Vec::new();

    for path in paths {
        let content = read_markdown_with_retry(path).await?;
        let chunks = chunk_markdown(&content, 800, 120);

        for (index, chunk) in chunks.into_iter().enumerate() {
            let embedding = embedder.embed(&chunk).await?;
            vectors.push(ChunkVector {
                source_path: path.display().to_string(),
                chunk_index: index,
                text: chunk,
                embedding,
            });
        }
    }

    qdrant.bulk_upsert(&vectors).await?;
    Ok(())
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
