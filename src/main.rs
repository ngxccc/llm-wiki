use tokio::task;

#[tokio::main]
async fn main() {
    let watcher = task::spawn(async {
        // Placeholder for the ingestion pipeline entrypoint.
    });

    let mcp_server = task::spawn(async {
        // Placeholder for the MCP server entrypoint.
    });

    let _ = tokio::join!(watcher, mcp_server);
}
