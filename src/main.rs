mod mcp;

use mcp::server::{McpServer, StaticSearchBackend};
use tokio::task;

#[tokio::main]
async fn main() {
    let watcher = task::spawn(async {
        // Placeholder for the ingestion pipeline entrypoint.
    });

    let mcp_server = task::spawn(async {
        let server = McpServer::new(StaticSearchBackend);
        let _ = server.run_stdio().await;
    });

    let _ = tokio::join!(watcher, mcp_server);
}
