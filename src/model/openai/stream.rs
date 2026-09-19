use std::time::{Duration, Instant};

use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};

use super::super::model::{
    FinishReason, Message, ModelError, ModelErrorKind, ModelRequest, ModelResponse, Usage,
};

#[derive(Clone)]
pub struct OpenAiStreamClient {
    http: Client,
    endpoint: Url,
    api_key: String,
    model: String,
}

impl OpenAiStreamClient {
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
        let mut response = self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(&self.api_key)
            .json(&ProviderRequest {
                model: &self.model,
                messages: &request.messages,
                max_tokens: request.limits.max_output_tokens,
                stream: true,
                stream_options: StreamOptions {
                    include_usage: true,
                },
            })
            .send()
            .await
            .map_err(map_transport_error)?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.map_err(map_transport_error)?;
            return Err(ModelError::from_status(status, &body));
        }

        let mut stream = StreamAccumulator::default();
        while let Some(chunk) = response.chunk().await.map_err(map_transport_error)? {
            stream.push(&chunk)?;
            if stream.done {
                break;
            }
        }
        stream.finish(started.elapsed().as_millis())
    }
}

fn map_transport_error(error: reqwest::Error) -> ModelError {
    ModelError::new(if error.is_timeout() {
        ModelErrorKind::Timeout
    } else {
        ModelErrorKind::ProviderUnavailable
    })
}

#[derive(Serialize)]
struct ProviderRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
    max_tokens: u64,
    stream: bool,
    stream_options: StreamOptions,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

struct StreamAccumulator {
    buffer: Vec<u8>,
    content: String,
    finish_reason: FinishReason,
    usage: Option<ProviderUsage>,
    done: bool,
}

impl Default for StreamAccumulator {
    fn default() -> Self {
        Self {
            buffer: Vec::new(),
            content: String::new(),
            finish_reason: FinishReason::Unknown,
            usage: None,
            done: false,
        }
    }
}

impl StreamAccumulator {
    fn push(&mut self, chunk: &[u8]) -> Result<(), ModelError> {
        if self.done {
            return Ok(());
        }
        self.buffer.extend_from_slice(chunk);
        while let Some(end) = event_end(&self.buffer) {
            let event = self.buffer.drain(..end).collect::<Vec<_>>();
            self.consume_event(&event)?;
            if self.done {
                self.buffer.clear();
                break;
            }
        }
        Ok(())
    }

    fn consume_event(&mut self, event: &[u8]) -> Result<(), ModelError> {
        let event = std::str::from_utf8(event)
            .map_err(|_| ModelError::new(ModelErrorKind::InvalidResponse))?;
        let data = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            return Ok(());
        }
        if data == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let chunk: ProviderChunk =
            serde_json::from_str(&data).map_err(|_| ModelError::invalid_response(&data))?;
        if let Some(error) = chunk.error {
            return Err(ModelError::new(provider_error_kind(&error)));
        }
        for choice in chunk.choices {
            if let Some(content) = choice.delta.content {
                self.content.push_str(&content);
            }
            if let Some(reason) = choice.finish_reason {
                self.finish_reason = normalize_finish_reason(&reason);
            }
        }
        if chunk.usage.is_some() {
            self.usage = chunk.usage;
        }
        Ok(())
    }

    fn finish(self, latency_ms: u128) -> Result<ModelResponse, ModelError> {
        if !self.done {
            return Err(ModelError::new(ModelErrorKind::ProviderUnavailable));
        }
        Ok(ModelResponse {
            content: (!self.content.is_empty()).then_some(self.content),
            tool_calls: Vec::new(),
            finish_reason: self.finish_reason,
            usage: normalize_usage(self.usage),
            latency_ms,
        })
    }
}

fn provider_error_kind(error: &serde_json::Value) -> ModelErrorKind {
    match error.get("code").and_then(serde_json::Value::as_str) {
        Some("invalid_api_key" | "authentication_error" | "authentication_failed") => {
            ModelErrorKind::AuthenticationFailed
        }
        Some("rate_limit_exceeded" | "rate_limited") => ModelErrorKind::RateLimited,
        Some("timeout") => ModelErrorKind::Timeout,
        Some("server_error" | "service_unavailable" | "provider_unavailable") => {
            ModelErrorKind::ProviderUnavailable
        }
        Some("context_length_exceeded") => ModelErrorKind::ContextTooLarge,
        _ => ModelErrorKind::InvalidResponse,
    }
}

fn event_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|position| position + 2)
        .or_else(|| {
            buffer
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| position + 4)
        })
}

#[derive(Deserialize)]
struct ProviderChunk {
    #[serde(default)]
    choices: Vec<ProviderChoice>,
    usage: Option<ProviderUsage>,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ProviderChoice {
    #[serde(default)]
    delta: ProviderDelta,
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct ProviderDelta {
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
