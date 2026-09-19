use std::time::{Duration, Instant};

use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};

use super::super::model::{
    FinishReason, Message, ModelError, ModelErrorKind, ModelRequest, ModelResponse, Usage,
};

#[derive(Clone)]
pub struct OpenAiChatClient {
    http: Client,
    endpoint: Url,
    api_key: String,
    model: String,
}

impl OpenAiChatClient {
    pub fn new(
        base_url: &str,
        api_key: impl Into<String>,
        model: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, ModelError> {
        let mut base_url = base_url.trim_end_matches('/').to_owned();
        base_url.push('/');
        let endpoint = Url::parse(&base_url)
            .and_then(|url| url.join("chat/completions"))
            .map_err(|_| ModelError::new(ModelErrorKind::InvalidResponse))?;
        let http = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|_| ModelError::new(ModelErrorKind::InvalidResponse))?;
        Ok(Self {
            http,
            endpoint,
            api_key: api_key.into(),
            model: model.into(),
        })
    }

    pub async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        let started = Instant::now();
        let response = self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(&self.api_key)
            .json(&ProviderRequest {
                model: &self.model,
                messages: &request.messages,
                max_tokens: request.limits.max_output_tokens,
            })
            .send()
            .await
            .map_err(|error| {
                ModelError::new(if error.is_timeout() {
                    ModelErrorKind::Timeout
                } else {
                    ModelErrorKind::ProviderUnavailable
                })
            })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|_| ModelError::new(ModelErrorKind::InvalidResponse))?;
        if !status.is_success() {
            return Err(ModelError::from_status(status, &body));
        }
        let response: ProviderResponse =
            serde_json::from_str(&body).map_err(|_| ModelError::invalid_response(&body))?;
        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| ModelError::invalid_response(&body))?;

        Ok(ModelResponse {
            content: choice.message.content,
            tool_calls: Vec::new(),
            finish_reason: normalize_finish_reason(&choice.finish_reason),
            usage: normalize_usage(response.usage),
            latency_ms: started.elapsed().as_millis(),
        })
    }
}

#[derive(Serialize)]
struct ProviderRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
    max_tokens: u64,
}

#[derive(Deserialize)]
struct ProviderResponse {
    choices: Vec<ProviderChoice>,
    usage: Option<ProviderUsage>,
}

#[derive(Deserialize)]
struct ProviderChoice {
    message: ProviderMessage,
    finish_reason: String,
}

#[derive(Deserialize)]
struct ProviderMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct ProviderUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    #[serde(default)]
    prompt_tokens_details: PromptTokenDetails,
}

#[derive(Default, Deserialize)]
struct PromptTokenDetails {
    #[serde(default)]
    cached_tokens: u64,
}

fn normalize_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Unknown,
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
