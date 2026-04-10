use anyhow::{Context, Result};
use qdrant_client::qdrant::vectors_config::Config;
use qdrant_client::qdrant::{value::Kind, PointStruct, SearchPoints, UpsertPoints, Value};
use qdrant_client::qdrant::{CreateCollection, Distance, VectorParams, VectorsConfig};
use qdrant_client::Qdrant;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ChunkVector {
    pub source_path: String,
    pub chunk_index: usize,
    pub text: String,
    pub embedding: Vec<f32>,
}

#[derive(Clone)]
pub struct QdrantStore {
    client: Qdrant,
    collection: String,
}

impl QdrantStore {
    pub fn new(url: &str, collection: String, api_key: Option<String>) -> Result<Self> {
        let final_url = if !url.contains(':') && url.contains("qdrant.io") {
            format!("{url}:6334")
        } else {
            url.to_string()
        };

        let mut builder = Qdrant::from_url(&final_url);
        builder = builder.skip_compatibility_check();

        if let Some(key) = api_key {
            builder = builder.api_key(key);
        }

        let client = builder
            .build()
            .context("failed to build qdrant client - check your URL and API Key")?;
        Ok(Self { client, collection })
    }

    /// Bulk upsert uses one gRPC request for all points in the batch.
    ///
    /// Time: O(n * d) for n points with d-dimension vectors.
    pub async fn bulk_upsert(&self, vectors: &[ChunkVector]) -> Result<()> {
        if vectors.is_empty() {
            return Ok(());
        }

        let mut points = Vec::with_capacity(vectors.len());
        for vector in vectors {
            let chunk_index =
                i64::try_from(vector.chunk_index).context("chunk index does not fit in i64")?;

            let mut payload = HashMap::new();
            payload.insert(
                "source_path".to_string(),
                Value::from(vector.source_path.clone()),
            );
            payload.insert("chunk_index".to_string(), Value::from(chunk_index));
            payload.insert("text".to_string(), Value::from(vector.text.clone()));

            points.push(PointStruct::new(
                Uuid::new_v4().to_string(),
                vector.embedding.clone(),
                payload,
            ));
        }

        self.client
            .upsert_points(UpsertPoints {
                collection_name: self.collection.clone(),
                wait: Some(true),
                points,
                ..Default::default()
            })
            .await
            .context("qdrant bulk upsert failed")?;

        Ok(())
    }

    /// Vector search complexity is O(log n) to O(n) internally depending on HNSW state.
    pub async fn search(&self, query_vector: Vec<f32>, limit: u64) -> Result<Vec<String>> {
        let response = self
            .client
            .search_points(SearchPoints {
                collection_name: self.collection.clone(),
                vector: query_vector,
                limit,
                with_payload: Some(true.into()),
                ..Default::default()
            })
            .await
            .context("qdrant search request failed")?;

        let texts = response
            .result
            .into_iter()
            .filter_map(|point| point.payload.get("text").cloned())
            .filter_map(|value| match value.kind {
                Some(Kind::StringValue(text)) => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>();

        Ok(texts)
    }

    /// Checks if the collection exists, creates it if not.
    /// Time Complexity: O(1) network call per startup. Space Complexity: O(1).
    pub async fn ensure_collection_exists(&self, dimension: u64) -> Result<()> {
        let exists = self.client.collection_exists(&self.collection).await?;

        if !exists {
            eprintln!(
                "Collection '{}' not found. Creating a new one...",
                self.collection
            );

            self.client
                .create_collection(CreateCollection {
                    collection_name: self.collection.clone(),
                    vectors_config: Some(VectorsConfig {
                        config: Some(Config::Params(VectorParams {
                            size: dimension,
                            // Use Cosine similarity for typical LLM embeddings
                            distance: Distance::Cosine.into(),
                            ..Default::default()
                        })),
                    }),
                    ..Default::default()
                })
                .await
                .context("Failed to create Qdrant collection")?;

            eprintln!("✨ Collection successfully created!");
        }
        Ok(())
    }
}
