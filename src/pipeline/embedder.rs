use crate::config::EmbeddingConfig;
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

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
