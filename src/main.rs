mod cache;
mod config;
mod db;
mod mcp;
mod pipeline;

use cache::semantic::SemanticCache;
use config::AppConfig;
use db::qdrant::QdrantStore;
use mcp::server::{CachedSearchBackend, McpServer, QdrantSearchProvider};
use pipeline::embedder::EmbeddingClient;
use pipeline::watcher::run_watcher;
use tokio::task;

#[tokio::main]
async fn main() {
    let config = AppConfig::load_or_create("config.yaml");

    let qdrant = match QdrantStore::new(&config.qdrant_url, config.qdrant_collection.clone()) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("failed to initialize qdrant client: {error}");
            return;
        }
    };

    let embedder = match EmbeddingClient::new(config.embedding.clone()) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("failed to initialize embedding client: {error}");
            return;
        }
    };

    let watcher_qdrant = qdrant.clone();
    let watcher_embedder = embedder.clone();
    let watcher_path = config.raw_data_path.clone();

    let watcher = task::spawn(async {
        if let Err(error) = run_watcher(watcher_path, watcher_embedder, watcher_qdrant).await {
            eprintln!("watcher loop exited with error: {error}");
        }
    });

    let provider = QdrantSearchProvider::new(qdrant, embedder);

    let mcp_server = task::spawn(async {
        let cache = SemanticCache::new(128, 64);
        let backend = CachedSearchBackend::new(cache, provider);
        let server = McpServer::new(backend);
        if let Err(error) = server.run_stdio().await {
            eprintln!("mcp server exited with error: {error}");
        }
    });

    let _ = tokio::join!(watcher, mcp_server);
}
