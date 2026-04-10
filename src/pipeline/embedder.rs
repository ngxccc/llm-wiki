use crate::config::EmbeddingConfig;
use anyhow::{Context, Result};
use base64::Engine;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::sleep;

#[derive(Clone)]
pub struct EmbeddingClient {
    client: Client,
    config: EmbeddingConfig,
}

#[derive(Serialize)]
struct ModernEmbeddingRequest<'a> {
    model: &'a str,
    input: Vec<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<usize>,
}

#[derive(Deserialize)]
struct ModernEmbeddingResponse {
    #[serde(default)]
    data: Vec<EmbeddingData>,
    #[serde(default)]
    embeddings: Vec<Vec<f32>>,
    #[serde(default)]
    embedding: Option<Vec<f32>>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    #[serde(default)]
    index: usize,
    embedding: EmbeddingValue,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum EmbeddingValue {
    FloatArray(Vec<f32>),
    Base64String(String),
}

impl EmbeddingClient {
    pub fn new(config: EmbeddingConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs.max(1)))
            .build()
            .context("failed to build reqwest client")?;

        Ok(Self { client, config })
    }

    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_with_retry(text, 3).await
    }

    pub fn max_batch_size(&self) -> usize {
        self.config.max_batch_size.max(1)
    }

    /// Calls the embedding API with exponential backoff.
    ///
    /// Time: O(r) network attempts, where r is `max_retries`.
    /// Space: O(1) additional memory.
    pub async fn embed_with_retry(&self, text: &str, max_retries: u32) -> Result<Vec<f32>> {
        let embeddings = self.embed_batch_with_retry(&[text], max_retries).await?;
        embeddings
            .into_iter()
            .next()
            .context("embedding API returned an empty batch")
    }

    /// Calls the embedding API in batch mode with exponential backoff.
    ///
    /// Time: O(r) network attempts, where r is `max_retries`.
    /// Space: O(n * d) for n texts and d-dimension vectors.
    pub async fn embed_batch_with_retry(
        &self,
        texts: &[&str],
        max_retries: u32,
    ) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut attempt = 0_u32;
        let base_delay = 2_u64;

        loop {
            match self.call_embedding_api_batch(texts).await {
                Ok(vectors) => return Ok(vectors),
                Err(error) => {
                    attempt += 1;
                    if attempt > max_retries {
                        eprintln!("embedding API retries exhausted: {error}");
                        return Err(error);
                    }

                    let wait_time = base_delay * 2_u64.pow(attempt - 1);
                    eprintln!(
                        "embedding API error on attempt {attempt}; retrying after {wait_time} seconds"
                    );
                    sleep(Duration::from_secs(wait_time)).await;
                }
            }
        }
    }

    async fn call_embedding_api_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let base = self.config.base_url.trim_end_matches('/');
        let endpoint = self.config.endpoint.trim_start_matches('/');
        let url = format!("{base}/{endpoint}");

        let payload = ModernEmbeddingRequest {
            model: &self.config.model,
            input: texts.to_vec(),
            dimensions: self.config.dimensions,
        };

        let mut request = self.client.post(url).json(&payload);
        if let Some(api_key) = self
            .config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            request = request.bearer_auth(api_key);
        }

        let response = request
            .send()
            .await
            .context("embedding request failed")?
            .error_for_status()
            .context("embedding server returned error status")?;

        let body = response
            .json::<ModernEmbeddingResponse>()
            .await
            .context("invalid embedding response payload")?;

        let embeddings = extract_embeddings(body)?;

        if embeddings.len() != texts.len() {
            anyhow::bail!(
                "embedding count mismatch: request has {} item(s) but response has {} item(s)",
                texts.len(),
                embeddings.len()
            );
        }

        Ok(embeddings)
    }
}

fn extract_embeddings(response: ModernEmbeddingResponse) -> Result<Vec<Vec<f32>>> {
    if !response.data.is_empty() {
        let mut data = response.data;
        data.sort_by_key(|item| item.index);

        let mut out = Vec::with_capacity(data.len());
        for item in data {
            match item.embedding {
                EmbeddingValue::FloatArray(values) => out.push(values),
                EmbeddingValue::Base64String(encoded) => {
                    out.push(decode_base64_embedding(&encoded)?);
                }
            }
        }

        return Ok(out);
    }

    if !response.embeddings.is_empty() {
        return Ok(response.embeddings);
    }

    if let Some(single) = response.embedding {
        return Ok(vec![single]);
    }

    anyhow::bail!("embedding response did not include any embeddings")
}

fn decode_base64_embedding(encoded: &str) -> Result<Vec<f32>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .context("failed to decode base64 embedding")?;

    if bytes.len() % 4 != 0 {
        anyhow::bail!(
            "invalid base64 embedding byte length {}; expected multiple of 4",
            bytes.len()
        );
    }

    let mut embedding = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        embedding.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }

    Ok(embedding)
}
