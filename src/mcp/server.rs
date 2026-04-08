use crate::cache::semantic::{CacheOutcome, SemanticCache};
use crate::db::qdrant::QdrantStore;
use crate::mcp::protocol::{
    CallToolResult, InitializeResult, JSON_RPC_VERSION, JsonRpcError, JsonRpcRequest, JsonRpcResponse, LATEST_PROTOCOL_VERSION, SearchWikiArguments, ServerCapabilities, ServerInfo, ToolContent, ToolDefinition, ToolsCallParams, ToolsCapability, ToolsListResult
};
use crate::pipeline::embedder::EmbeddingClient;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};
use std::fmt::{Display, Formatter};
use std::io;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

const SERVER_NAME: &str = "llm-wiki";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const SEARCH_TOOL_NAME: &str = "search_wiki";

pub struct McpServer<S> {
    search_backend: S,
}

impl<S> McpServer<S> {
    pub fn new(search_backend: S) -> Self {
        Self { search_backend }
    }
}

impl<S> McpServer<S>
where
    S: WikiSearchBackend,
{
    pub async fn run_stdio(self) -> Result<(), McpError> {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        self.run(BufReader::new(stdin), stdout).await
    }

    pub async fn run<R, W>(self, mut reader: BufReader<R>, mut writer: W) -> Result<(), McpError>
    where
        R: tokio::io::AsyncRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
    {
        loop {
            let Some(request) = read_request(&mut reader).await? else {
                eprintln!("MCP client closed stdin (EOF). Shutting down process.");
                std::process::exit(0);
            };

            if request.id.is_none() {
                continue;
            }

            let response = self.handle_request(request).await;
            write_response(&mut writer, response).await?;
        }
    }

    async fn handle_request(&self, request: JsonRpcRequest) -> JsonRpcResponse<Value> {
        let id = request.id.unwrap_or(Value::Null);

        if request.jsonrpc != JSON_RPC_VERSION {
            return JsonRpcResponse {
                jsonrpc: JSON_RPC_VERSION,
                id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32600,
                    message: "Invalid JSON-RPC version".to_string(),
                    data: None,
                }),
            };
        }

        match request.method.as_str() {
            "initialize" => {
                let result = InitializeResult {
                    protocol_version: LATEST_PROTOCOL_VERSION.to_string(),
                    capabilities: ServerCapabilities {
                        tools: ToolsCapability {
                            list_changed: Some(false),
                        },
                    },
                    server_info: ServerInfo {
                        name: SERVER_NAME.to_string(),
                        version: SERVER_VERSION.to_string(),
                    },
                };

                JsonRpcResponse {
                    jsonrpc: JSON_RPC_VERSION,
                    id,
                    result: Some(serde_json::to_value(result).unwrap_or(Value::Null)),
                    error: None,
                }
            }
            "tools/list" => {
                let result = ToolsListResult {
                    tools: vec![ToolDefinition {
                        name: SEARCH_TOOL_NAME.to_string(),
                        description: "Search the local wiki for relevant chunks".to_string(),
                        input_schema: json!({
                            "type": "object",
                            "properties": {
                                "query": { "type": "string" }
                            },
                            "required": ["query"],
                            "additionalProperties": false
                        }),
                    }],
                };

                JsonRpcResponse {
                    jsonrpc: JSON_RPC_VERSION,
                    id,
                    result: Some(serde_json::to_value(result).unwrap_or(Value::Null)),
                    error: None,
                }
            }
            "tools/call" => self.handle_call_tool(id, request.params).await,
            _ => JsonRpcResponse {
                jsonrpc: JSON_RPC_VERSION,
                id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: "Method not found".to_string(),
                    data: None,
                }),
            },
        }
    }

    async fn handle_call_tool(&self, id: Value, params: Option<Value>) -> JsonRpcResponse<Value> {
        let parsed = params
            .and_then(|value| serde_json::from_value::<ToolsCallParams>(value).ok())
            .ok_or("Invalid tools/call request")
            .and_then(|call_params| {
                if call_params.name != SEARCH_TOOL_NAME {
                    return Err("Unsupported tool");
                }

                let arguments = call_params
                    .arguments
                    .ok_or("Missing tool arguments")
                    .and_then(|value| {
                        serde_json::from_value::<SearchWikiArguments>(value)
                            .map_err(|_| "Invalid query argument")
                    })?;

                Ok(arguments.query)
            });

        let query = match parsed {
            Ok(query) => query,
            Err(message) => {
                return JsonRpcResponse {
                    jsonrpc: JSON_RPC_VERSION,
                    id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: message.to_string(),
                        data: None,
                    }),
                };
            }
        };

        let search_result = self.search_backend.search(&query).await;

        match search_result {
            Ok(summary) => {
                let content = vec![ToolContent {
                    kind: "text".to_string(),
                    text: summary,
                }];

                let result = CallToolResult {
                    content,
                    is_error: Some(false),
                };

                JsonRpcResponse {
                    jsonrpc: JSON_RPC_VERSION,
                    id,
                    result: Some(serde_json::to_value(result).unwrap_or(Value::Null)),
                    error: None,
                }
            }
            Err(error) => JsonRpcResponse {
                jsonrpc: JSON_RPC_VERSION,
                id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32000,
                    message: error.to_string(),
                    data: None,
                }),
            },
        }
    }
}

#[async_trait]
pub trait WikiSearchBackend {
    async fn search(&self, query: &str) -> Result<String, McpError>;
}

#[async_trait]
pub trait FreshSearchProvider {
    async fn fetch(&self, query: &str) -> Result<String, McpError>;
}

#[derive(Clone)]
pub struct QdrantSearchProvider {
    qdrant: QdrantStore,
    embedder: EmbeddingClient,
}

impl QdrantSearchProvider {
    pub fn new(qdrant: QdrantStore, embedder: EmbeddingClient) -> Self {
        Self { qdrant, embedder }
    }
}

pub struct CachedSearchBackend<P> {
    cache: SemanticCache<String>,
    provider: P,
}

impl<P> CachedSearchBackend<P> {
    pub fn new(cache: SemanticCache<String>, provider: P) -> Self {
        Self { cache, provider }
    }
}

#[async_trait]
impl<P> WikiSearchBackend for CachedSearchBackend<P>
where
    P: FreshSearchProvider + Send + Sync,
{
    async fn search(&self, query: &str) -> Result<String, McpError> {
        let embedding = embed_query(query, self.cache.vector_dimension());

        match self.cache.probe(query, &embedding) {
            CacheOutcome::SureHit { value } | CacheOutcome::GreyZone { value } => {
                Ok((*value).clone())
            }
            CacheOutcome::Miss => {
                let fresh = self.provider.fetch(query).await?;
                self.cache.insert(query, &embedding, fresh.clone());
                Ok(fresh)
            }
        }
    }
}

#[async_trait]
impl FreshSearchProvider for QdrantSearchProvider {
    async fn fetch(&self, query: &str) -> Result<String, McpError> {
        let query_embedding =
            self.embedder.embed(query).await.map_err(|error| {
                McpError::External(format!("embedding request failed: {error}"))
            })?;

        let search_result = self
            .qdrant
            .search(query_embedding, 5)
            .await
            .map_err(|error| McpError::External(format!("qdrant search failed: {error}")))?;

        if search_result.is_empty() {
            Ok("No wiki chunks matched the query.".to_string())
        } else {
            Ok(search_result.join("\n---\n"))
        }
    }
}

#[derive(Debug)]
pub enum McpError {
    Io(io::Error),
    Json(serde_json::Error),
    External(String),
}

impl Display for McpError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Json(error) => write!(f, "JSON error: {error}"),
            Self::External(error) => write!(f, "External error: {error}"),
        }
    }
}

impl std::error::Error for McpError {}

impl From<io::Error> for McpError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for McpError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

async fn read_request<R>(reader: &mut BufReader<R>) -> Result<Option<JsonRpcRequest>, McpError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut content_length = None;

    loop {
        let mut line = String::new();
        let bytes_read = match reader.read_line(&mut line).await {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("Failed to read MCP stdin: {error}. Exiting process.");
                std::process::exit(1);
            }
        };
        if bytes_read == 0 {
            return Ok(None);
        }

        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }

        if let Some((name, value)) = trimmed.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
                );
            }
        }
    }

    let body_length = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "Missing Content-Length header")
    })?;
    let mut buffer = vec![0_u8; body_length];
    reader.read_exact(&mut buffer).await?;

    Ok(Some(serde_json::from_slice(&buffer)?))
}

async fn write_response<W, T>(writer: &mut W, response: JsonRpcResponse<T>) -> Result<(), McpError>
where
    W: tokio::io::AsyncWrite + Unpin,
    T: Serialize,
{
    let body = serde_json::to_vec(&response)?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

fn embed_query(query: &str, dimensions: usize) -> Vec<f32> {
    let mut vector = vec![0.0_f32; dimensions.max(1)];

    for (index, byte) in query.bytes().enumerate() {
        let slot = index % vector.len();
        vector[slot] += (f32::from(byte) / 255.0) * 2.0 - 1.0;
    }

    let norm = vector
        .iter()
        .map(|component| component * component)
        .sum::<f32>()
        .sqrt();
    if norm > 0.0 {
        for component in &mut vector {
            *component /= norm;
        }
    }

    vector
}
