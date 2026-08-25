//! Local provider discovery. Harnesses remain responsible for model calls.

use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredProvider {
    pub kind: String,
    pub base_url: String,
    pub healthy: bool,
    pub models: Vec<ModelDescriptor>,
    pub latency_ms: u128,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDescriptor {
    pub id: String,
    pub owned_by: Option<String>,
    pub context_window: Option<u64>,
    pub vision: Option<bool>,
    pub raw: Value,
}

#[derive(Clone)]
pub struct ProviderDiscovery {
    client: Client,
}

impl ProviderDiscovery {
    pub fn new() -> Result<Self, ProviderError> {
        Ok(Self {
            client: Client::builder()
                .timeout(Duration::from_secs(4))
                .no_proxy()
                .build()?,
        })
    }

    pub async fn lm_studio(&self, base_url: &str) -> Result<DiscoveredProvider, ProviderError> {
        let base = normalize_openai_base(base_url)?;
        let url = base
            .join("models")
            .map_err(|error| ProviderError::InvalidEndpoint(error.to_string()))?;
        let started = std::time::Instant::now();
        let response = self.client.get(url).send().await?.error_for_status()?;
        let body: OpenAiModels = response.json().await?;
        let models = body
            .data
            .into_iter()
            .map(|raw| ModelDescriptor {
                id: raw.id,
                owned_by: raw.owned_by,
                context_window: raw
                    .extra
                    .get("max_context_length")
                    .or_else(|| raw.extra.get("context_length"))
                    .and_then(Value::as_u64),
                vision: infer_vision(&raw.extra),
                raw: Value::Object(raw.extra),
            })
            .collect();
        Ok(DiscoveredProvider {
            kind: "lm-studio".into(),
            base_url: base.to_string().trim_end_matches('/').into(),
            healthy: true,
            models,
            latency_ms: started.elapsed().as_millis(),
            note: None,
        })
    }

    pub async fn ollama(&self, endpoint: &str) -> Result<DiscoveredProvider, ProviderError> {
        let base = Url::parse(endpoint)
            .map_err(|error| ProviderError::InvalidEndpoint(error.to_string()))?;
        let url = base
            .join("api/tags")
            .map_err(|error| ProviderError::InvalidEndpoint(error.to_string()))?;
        let started = std::time::Instant::now();
        let response = self.client.get(url).send().await?.error_for_status()?;
        let body: OllamaTags = response.json().await?;
        let models = body
            .models
            .into_iter()
            .map(|model| ModelDescriptor {
                id: model.name,
                owned_by: Some("ollama".into()),
                context_window: None,
                vision: None,
                raw: model.extra,
            })
            .collect();
        Ok(DiscoveredProvider {
            kind: "ollama".into(),
            base_url: format!("{}/v1", endpoint.trim_end_matches('/')),
            healthy: true,
            models,
            latency_ms: started.elapsed().as_millis(),
            note: None,
        })
    }

    pub async fn discover_defaults(&self) -> Vec<DiscoveredProvider> {
        let (lm, ollama) = tokio::join!(
            self.lm_studio("http://127.0.0.1:1234/v1"),
            self.ollama("http://127.0.0.1:11434")
        );
        [lm, ollama].into_iter().filter_map(Result::ok).collect()
    }
}

fn normalize_openai_base(value: &str) -> Result<Url, ProviderError> {
    let mut value = value.trim_end_matches('/').to_string();
    if !value.ends_with("/v1") {
        value.push_str("/v1");
    }
    value.push('/');
    Url::parse(&value).map_err(|error| ProviderError::InvalidEndpoint(error.to_string()))
}

fn infer_vision(extra: &serde_json::Map<String, Value>) -> Option<bool> {
    extra
        .get("capabilities")
        .and_then(|value| value.get("vision"))
        .and_then(Value::as_bool)
        .or_else(|| extra.get("vision").and_then(Value::as_bool))
}

#[derive(Debug, Deserialize)]
struct OpenAiModels {
    data: Vec<OpenAiModel>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModel {
    id: String,
    owned_by: Option<String>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct OllamaTags {
    models: Vec<OllamaModel>,
}

#[derive(Debug, Deserialize)]
struct OllamaModel {
    name: String,
    #[serde(flatten)]
    extra: Value,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("VCTR_PROVIDER_UNAVAILABLE: request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("VCTR_PROVIDER_UNAVAILABLE: invalid endpoint: {0}")]
    InvalidEndpoint(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_openai_endpoints() {
        assert_eq!(
            normalize_openai_base("http://localhost:1234")
                .unwrap()
                .as_str(),
            "http://localhost:1234/v1/"
        );
        assert_eq!(
            normalize_openai_base("http://localhost:1234/v1/")
                .unwrap()
                .as_str(),
            "http://localhost:1234/v1/"
        );
    }
}
