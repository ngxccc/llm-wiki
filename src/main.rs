mod cache;
mod config;
mod db;
mod mcp;
mod pipeline;

use anyhow::Context;
use cache::semantic::SemanticCache;
use config::AppConfig;
use db::qdrant::QdrantStore;
use mcp::server::{CachedSearchBackend, McpServer, QdrantSearchProvider};
use pipeline::embedder::EmbeddingClient;
use pipeline::watcher::run_watcher;
use tokio::task;
use tokio::time::{timeout, Duration};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = AppConfig::load_or_create("config.yaml");
    let cancel_token = CancellationToken::new();

    let qdrant = match QdrantStore::new(
        &config.qdrant_url,
        config.qdrant_collection.clone(),
        config.qdrant_api_key.clone(),
    ) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("failed to initialize qdrant client: {error}");
            return Ok(());
        }
    };

    let vector_dim = config.embedding.dimensions.unwrap_or(1024); // Fallback
    let vector_dim_u64 =
        u64::try_from(vector_dim).context("Embedding dimension conversion failed")?;

    qdrant
        .ensure_collection_exists(vector_dim_u64)
        .await
        .context("Init DB failed")?;

    let embedder = match EmbeddingClient::new(config.embedding.clone()) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("failed to initialize embedding client: {error}");
            return Ok(());
        }
    };

    let watcher_qdrant = qdrant.clone();
    let watcher_embedder = embedder.clone();
    let watcher_path = config.raw_data_path.clone();
    let watcher_token = cancel_token.clone();

    let watcher = task::spawn(async move {
        if let Err(error) = run_watcher(
            watcher_path,
            watcher_embedder,
            watcher_qdrant,
            watcher_token,
            vector_dim,
        )
        .await
        {
            eprintln!("watcher loop exited with error: {error}");
        }
    });

    let provider = QdrantSearchProvider::new(qdrant, embedder, vector_dim);
    let mcp_token = cancel_token.clone();

    let mcp_server = task::spawn(async move {
        let cache = SemanticCache::new(128, vector_dim);
        let backend = CachedSearchBackend::new(cache, provider);
        let server = McpServer::new(backend);
        tokio::select! {
            result = server.run_stdio() => {
                if let Err(error) = result {
                    eprintln!("mcp server exited with error: {error}");
                }
            }
            () = mcp_token.cancelled() => {
                eprintln!("MCP server received shutdown signal.");
            }
        }
    });

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("failed to install SIGTERM handler: {error}");
                std::process::exit(1);
            }
        };

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                eprintln!("Shutdown signal received (Ctrl+C). Starting graceful shutdown.");
            }
            _ = terminate.recv() => {
                eprintln!("Shutdown signal received (SIGTERM). Starting graceful shutdown.");
            }
        }
    }

    #[cfg(not(unix))]
    {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("Shutdown signal received (Ctrl+C). Starting graceful shutdown.");
        }
    }

    cancel_token.cancel();

    eprintln!("Waiting up to 3 seconds for background tasks to finish cleanup.");
    let wait_tasks_future = async {
        let _ = tokio::join!(watcher, mcp_server);
    };

    match timeout(Duration::from_secs(3), wait_tasks_future).await {
        Ok(()) => {
            eprintln!("Graceful shutdown complete.");
        }
        Err(_) => {
            eprintln!("Graceful shutdown timed out after 3 seconds. Exiting now.");
        }
    }

    Ok(())
}
