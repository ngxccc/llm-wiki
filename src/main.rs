mod cache;
mod mcp;

use cache::semantic::SemanticCache;
use mcp::server::{CachedSearchBackend, McpServer, StaticSearchBackend};
use tokio::task;

#[tokio::main]
async fn main() {
    let watcher = task::spawn(async {
        // Placeholder for the ingestion pipeline entrypoint.
    });

    let mcp_server = task::spawn(async {
        let cache = SemanticCache::new(128, 64);
        let backend = CachedSearchBackend::new(cache, StaticSearchBackend);
        let server = McpServer::new(backend);
        let _ = server.run_stdio().await;
    });

    let _ = tokio::join!(watcher, mcp_server);
}
