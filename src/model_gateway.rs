use std::{env, time::Instant};

use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};

#[path = "model/mod.rs"]
pub mod model;

use model::{ModelError, Usage};

#[derive(Clone)]
pub struct OpenAiClient {
    http: Client,
    base_url: Url,
    api_key: String,
    model: String,
}

#[derive(Serialize)]
pub struct ProbeResult {
    pub status: &'static str,
    pub model: String,
    pub latency_ms: u128,
    pub content: String,
    pub usage: Option<Usage>,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    usage: Option<ProviderUsage>,
}

#[derive(Deserialize)]
struct ProviderUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<String>,
}

impl OpenAiClient {
    pub fn from_env() -> anyhow::Result<Self> {
        let base_url = normalize_base_url(&env::var("PRIMARY_BASE_URL")?)?;
        Ok(Self {
            http: Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()?,
            base_url,
            api_key: env::var("PRIMARY_API_KEY")?,
            model: env::var("PRIMARY_MODEL")?,
        })
    }

    pub async fn probe(&self) -> Result<ProbeResult, ModelError> {
        let started = Instant::now();
        let response = self
            .http
            .post(
                self.base_url
                    .join("chat/completions")
                    .map_err(|_| ModelError::new(model::ModelErrorKind::InvalidResponse))?,
            )
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "model": self.model,
                "messages": [{"role": "user", "content": "Reply with exactly: OK"}],
                "max_tokens": 8,
                "temperature": 0
            }))
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    ModelError::new(model::ModelErrorKind::Timeout)
                } else {
                    ModelError::new(model::ModelErrorKind::ProviderUnavailable)
                }
            })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|_| ModelError::new(model::ModelErrorKind::InvalidResponse))?;
        if !status.is_success() {
            return Err(ModelError::from_status(status, &body));
        }

        let parsed: ChatResponse =
            serde_json::from_str(&body).map_err(|_| ModelError::invalid_response(&body))?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content)
            .ok_or_else(|| ModelError::invalid_response(&body))?;

        Ok(ProbeResult {
            status: "ok",
            model: self.model.clone(),
            latency_ms: started.elapsed().as_millis(),
            content,
            usage: parsed.usage.map(|usage| Usage {
                input_tokens: usage.prompt_tokens,
                output_tokens: usage.completion_tokens,
                total_tokens: usage.total_tokens,
            }),
        })
    }
}
fn normalize_base_url(value: &str) -> anyhow::Result<Url> {
    let mut normalized = value.trim_end_matches('/').to_owned();
    normalized.push('/');
    Ok(Url::parse(&normalized)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_openai_endpoint_without_losing_v1() {
        let base = normalize_base_url("https://example.com/v1").unwrap();
        assert_eq!(
            base.join("chat/completions").unwrap().as_str(),
            "https://example.com/v1/chat/completions"
        );
    }
}
