use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub raw_data_path: PathBuf,
    pub qdrant_url: String,
    pub qdrant_collection: String,
    pub embedding: EmbeddingConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddingConfig {
    pub base_url: String,
    pub endpoint: String,
    pub model: String,
    pub timeout_secs: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            raw_data_path: PathBuf::from("data/raw"),
            qdrant_url: "http://127.0.0.1:6334".to_string(),
            qdrant_collection: "llm_wiki_chunks".to_string(),
            embedding: EmbeddingConfig {
                base_url: "http://127.0.0.1:11434".to_string(),
                endpoint: "/api/embeddings".to_string(),
                model: "nomic-embed-text".to_string(),
                timeout_secs: 30,
            },
        }
    }
}
