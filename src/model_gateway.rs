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
    pub usage: Usage,
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
    #[serde(default)]
    prompt_tokens_details: ProviderPromptTokenDetails,
}

#[derive(Default, Deserialize)]
struct ProviderPromptTokenDetails {
    #[serde(default)]
    cached_tokens: u64,
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
            usage: normalize_usage(parsed.usage),
        })
    }
}

fn normalize_usage(usage: Option<ProviderUsage>) -> Usage {
    usage.map_or(
        Usage {
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            total_tokens: 0,
            estimated: true,
        },
        |usage| Usage {
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            cached_tokens: usage.prompt_tokens_details.cached_tokens,
            total_tokens: usage.total_tokens,
            estimated: false,
        },
    )
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

    #[test]
    fn missing_usage_is_estimated() {
        let usage = normalize_usage(None);

        assert!(usage.estimated);
        assert_eq!(usage.total_tokens, 0);
    }

    #[test]
    fn provider_usage_is_measured_and_normalizes_cached_tokens() {
        let usage = normalize_usage(Some(ProviderUsage {
            prompt_tokens: 12,
            completion_tokens: 3,
            total_tokens: 15,
            prompt_tokens_details: ProviderPromptTokenDetails { cached_tokens: 4 },
        }));

        assert!(!usage.estimated);
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.output_tokens, 3);
        assert_eq!(usage.cached_tokens, 4);
        assert_eq!(usage.total_tokens, 15);
    }
}
