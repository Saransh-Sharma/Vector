//! Local provider discovery. Harnesses remain responsible for model calls.

use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use thiserror::Error;
use tokio::{process::Command, time::sleep};

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

    /// Discover LM Studio and, for loopback endpoints only, start its local API
    /// server through the official `lms` CLI when it is installed but stopped.
    pub async fn ensure_lm_studio(
        &self,
        base_url: &str,
    ) -> Result<DiscoveredProvider, ProviderError> {
        match self.lm_studio(base_url).await {
            Ok(provider) => return Ok(provider),
            Err(error) if !is_loopback_endpoint(base_url)? => return Err(error),
            Err(_) => {}
        }

        let cli = find_lms_cli().ok_or(ProviderError::LmStudioCliUnavailable)?;
        let port = endpoint_port(base_url)?;
        let output = Command::new(&cli)
            .args([
                "server",
                "start",
                "--port",
                &port.to_string(),
                "--bind",
                "127.0.0.1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|error| ProviderError::LmStudioStartFailed(error.to_string()))?;

        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(ProviderError::LmStudioStartFailed(if detail.is_empty() {
                format!("lms exited with {}", output.status)
            } else {
                detail
            }));
        }

        // `lms server start` normally returns only after binding, but the retry
        // window also covers slower first starts and desktop IPC initialization.
        for _ in 0..40 {
            if let Ok(mut provider) = self.lm_studio(base_url).await {
                provider.note = Some(format!(
                    "Vector started LM Studio's loopback server with {}",
                    cli.display()
                ));
                return Ok(provider);
            }
            sleep(Duration::from_millis(250)).await;
        }

        Err(ProviderError::LmStudioStartTimedOut { port })
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

fn endpoint_port(value: &str) -> Result<u16, ProviderError> {
    Ok(normalize_openai_base(value)?
        .port_or_known_default()
        .unwrap_or(1234))
}

fn is_loopback_endpoint(value: &str) -> Result<bool, ProviderError> {
    let endpoint = normalize_openai_base(value)?;
    let Some(host) = endpoint.host_str() else {
        return Ok(false);
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Ok(host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback()))
}

fn find_lms_cli() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("LMS_CLI_PATH").map(PathBuf::from)
        && executable_file(&path)
    {
        return Some(path);
    }

    let executable = if cfg!(windows) { "lms.exe" } else { "lms" };
    if let Some(path) = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|path| path.join(executable))
            .find(|path| executable_file(path))
    }) {
        return Some(path);
    }

    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)?;
    [
        home.join(".lmstudio/bin").join(executable),
        home.join(".local/bin").join(executable),
    ]
    .into_iter()
    .find(|path| executable_file(path))
}

fn executable_file(path: &Path) -> bool {
    path.metadata().is_ok_and(|metadata| metadata.is_file())
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
    #[error(
        "VCTR_LM_STUDIO_CLI_UNAVAILABLE: LM Studio is not reachable and its `lms` CLI was not found. Install the LM Studio CLI from the app's Developer tab."
    )]
    LmStudioCliUnavailable,
    #[error("VCTR_LM_STUDIO_START_FAILED: the LM Studio server could not be started: {0}")]
    LmStudioStartFailed(String),
    #[error(
        "VCTR_LM_STUDIO_START_TIMEOUT: LM Studio did not become healthy on loopback port {port} within 10 seconds"
    )]
    LmStudioStartTimedOut { port: u16 },
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

    #[test]
    fn autostart_is_restricted_to_loopback() {
        assert!(is_loopback_endpoint("http://127.0.0.1:1234/v1").unwrap());
        assert!(is_loopback_endpoint("http://localhost:1234").unwrap());
        assert!(is_loopback_endpoint("http://[::1]:1234/v1").unwrap());
        assert!(!is_loopback_endpoint("https://models.example.com/v1").unwrap());
    }

    #[test]
    fn preserves_the_configured_port() {
        assert_eq!(endpoint_port("http://127.0.0.1:54321/v1").unwrap(), 54321);
    }
}
