use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    pub raw_data_path: PathBuf,
    pub qdrant_url: String,
    pub qdrant_collection: String,
    pub embedding: EmbeddingConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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

impl AppConfig {
    pub fn load_or_create(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();

        match std::fs::read_to_string(path) {
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
        }
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
