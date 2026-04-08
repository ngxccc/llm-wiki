use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

const RAW_DATA_PATH_ENV: &str = "LLM_WIKI_RAW_DATA_PATH";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AppConfig {
    #[serde(default = "default_raw_data_path")]
    pub raw_data_path: PathBuf,
    #[serde(default = "default_qdrant_url")]
    pub qdrant_url: String,
    #[serde(default = "default_qdrant_collection")]
    pub qdrant_collection: String,
    #[serde(default)]
    pub embedding: EmbeddingConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct EmbeddingConfig {
    #[serde(default = "default_embedding_base_url")]
    pub base_url: String,
    #[serde(default = "default_embedding_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_embedding_model")]
    pub model: String,
    #[serde(default = "default_embedding_timeout_secs")]
    pub timeout_secs: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            raw_data_path: default_raw_data_path(),
            qdrant_url: default_qdrant_url(),
            qdrant_collection: default_qdrant_collection(),
            embedding: EmbeddingConfig::default(),
        }
    }
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            base_url: default_embedding_base_url(),
            endpoint: default_embedding_endpoint(),
            model: default_embedding_model(),
            timeout_secs: default_embedding_timeout_secs(),
        }
    }
}

impl AppConfig {
    pub fn load_or_create(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();

        let mut config = match std::fs::read_to_string(path) {
            Ok(content) => match serde_yaml::from_str::<Self>(&content) {
                Ok(config) => config,
                Err(error) => {
                    eprintln!(
                        "warning: failed to parse {} ({error}); falling back to default config",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let default_config = Self::default();
                if let Err(write_error) = write_template(path, &default_config) {
                    eprintln!(
                        "warning: {} not found; failed to create template ({write_error}); using default config",
                        path.display()
                    );
                } else {
                    eprintln!(
                        "warning: {} not found; generated template and using default config",
                        path.display()
                    );
                }

                default_config
            }
            Err(error) => {
                eprintln!(
                    "warning: failed to read {} ({error}); falling back to default config",
                    path.display()
                );
                Self::default()
            }
        };

        if let Some(raw_data_path) = raw_data_path_from_env() {
            config.raw_data_path = raw_data_path;
        }

        config
    }
}

fn write_template(path: &Path, config: &AppConfig) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let body = serde_yaml::to_string(config)
        .map_err(|error| io::Error::other(format!("failed to serialize template: {error}")))?;
    std::fs::write(path, body)
}

fn default_raw_data_path() -> PathBuf {
    PathBuf::from("data/raw")
}

fn default_qdrant_url() -> String {
    "http://127.0.0.1:6334".to_string()
}

fn default_qdrant_collection() -> String {
    "llm_wiki_chunks".to_string()
}

fn default_embedding_base_url() -> String {
    "http://127.0.0.1:11434".to_string()
}

fn default_embedding_endpoint() -> String {
    "/api/embeddings".to_string()
}

fn default_embedding_model() -> String {
    "nomic-embed-text".to_string()
}

fn default_embedding_timeout_secs() -> u64 {
    30
}

fn raw_data_path_from_env() -> Option<PathBuf> {
    match std::env::var_os(RAW_DATA_PATH_ENV) {
        Some(value) if !value.is_empty() => Some(PathBuf::from(value)),
        _ => None,
    }
}
