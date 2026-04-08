use anyhow::{Context, Result};
use qdrant_client::qdrant::{value::Kind, PointStruct, SearchPoints, UpsertPoints, Value};
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
    pub fn new(url: &str, collection: String) -> Result<Self> {
        let client = Qdrant::from_url(url)
            .build()
            .context("failed to build qdrant client")?;
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
            .context("qdrant search failed")?;

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
}
