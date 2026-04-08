use crate::config::EmbeddingConfig;
use anyhow::{Context, Result};
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
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a str,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    embedding: Vec<f32>,
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

    /// Calls the embedding API with exponential backoff.
    ///
    /// Time: O(r) network attempts, where r is `max_retries`.
    /// Space: O(1) additional memory.
    pub async fn embed_with_retry(&self, text: &str, max_retries: u32) -> Result<Vec<f32>> {
        let mut attempt = 0_u32;
        let base_delay = 2_u64;

        loop {
            match self.call_embedding_api(text).await {
                Ok(vector) => return Ok(vector),
                Err(error) => {
                    attempt += 1;
                    if attempt > max_retries {
                        eprintln!("💀 Đã đưa file vào DLQ vì Ollama sập: {error}");
                        return Err(error);
                    }

                    let wait_time = base_delay * 2_u64.pow(attempt - 1);
                    eprintln!("⚠️ Lỗi API. Thử lại lần {attempt} sau {wait_time} giây...");
                    sleep(Duration::from_secs(wait_time)).await;
                }
            }
        }
    }

    async fn call_embedding_api(&self, text: &str) -> Result<Vec<f32>> {
        let base = self.config.base_url.trim_end_matches('/');
        let endpoint = self.config.endpoint.trim_start_matches('/');
        let url = format!("{base}/{endpoint}");

        let payload = EmbeddingRequest {
            model: &self.config.model,
            input: text,
        };

        let response = self
            .client
            .post(url)
            .json(&payload)
            .send()
            .await
            .context("embedding request failed")?
            .error_for_status()
            .context("embedding server returned error status")?;

        let body = response
            .json::<EmbeddingResponse>()
            .await
            .context("invalid embedding response payload")?;

        Ok(body.embedding)
    }
}
