use std::time::{Duration, Instant};

use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::model::{
    FinishReason, Message, MessageRole, ModelError, ModelErrorKind, ModelLimits, ModelRequest,
    ModelResponse, ToolCall, ToolDefinition, Usage,
};

const PROBE_TOOL_NAME: &str = "noctis_capability_probe";

#[derive(Clone)]
pub struct OpenAiToolsClient {
    http: Client,
    endpoint: Url,
    api_key: String,
    model: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolProbeResult {
    Supported(ToolCall),
    Unsupported,
}

impl OpenAiToolsClient {
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
            .redirect(reqwest::redirect::Policy::none())
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
        let tools: Vec<_> = request.tools.iter().map(ProviderTool::from).collect();
        let response = self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(&self.api_key)
            .json(&ProviderRequest {
                model: &self.model,
                messages: &request.messages,
                tools: &tools,
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
        let tool_calls = normalize_tool_calls(choice.message.tool_calls, &body)?;

        Ok(ModelResponse {
            content: choice.message.content,
            tool_calls,
            finish_reason: normalize_finish_reason(&choice.finish_reason),
            usage: normalize_usage(response.usage),
            latency_ms: started.elapsed().as_millis(),
        })
    }

    pub async fn probe(&self) -> Result<ToolProbeResult, ModelError> {
        let response = self
            .complete(&ModelRequest {
                project_id: "probe".to_owned(),
                task_id: "probe".to_owned(),
                agent_run_id: "probe".to_owned(),
                model_class: "probe".to_owned(),
                messages: vec![Message {
                    role: MessageRole::User,
                    content: format!(
                        "Call {PROBE_TOOL_NAME} with enabled=true. Do not answer in text."
                    ),
                    tool_call_id: None,
                }],
                tools: vec![ToolDefinition {
                    name: PROBE_TOOL_NAME.to_owned(),
                    description: "Checks tool-call response compatibility without side effects."
                        .to_owned(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"enabled": {"type": "boolean"}},
                        "required": ["enabled"],
                        "additionalProperties": false
                    }),
                }],
                limits: ModelLimits {
                    max_input_tokens: 256,
                    max_output_tokens: 32,
                },
            })
            .await?;
        Ok(response
            .tool_calls
            .into_iter()
            .find(|call| call.name == PROBE_TOOL_NAME && call.arguments == json!({"enabled": true}))
            .map_or(ToolProbeResult::Unsupported, ToolProbeResult::Supported))
    }
}

#[derive(Serialize)]
struct ProviderRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
    tools: &'a [ProviderTool<'a>],
    max_tokens: u64,
}

#[derive(Serialize)]
struct ProviderTool<'a> {
    r#type: &'static str,
    function: ProviderFunction<'a>,
}

#[derive(Serialize)]
struct ProviderFunction<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a Value,
}

impl<'a> From<&'a ToolDefinition> for ProviderTool<'a> {
    fn from(tool: &'a ToolDefinition) -> Self {
        Self {
            r#type: "function",
            function: ProviderFunction {
                name: &tool.name,
                description: &tool.description,
                parameters: &tool.input_schema,
            },
        }
    }
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
    #[serde(default)]
    tool_calls: Vec<ProviderToolCall>,
}

#[derive(Deserialize)]
struct ProviderToolCall {
    id: String,
    function: ProviderToolCallFunction,
}

#[derive(Deserialize)]
struct ProviderToolCallFunction {
    name: String,
    arguments: String,
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

fn normalize_tool_calls(
    calls: Vec<ProviderToolCall>,
    body: &str,
) -> Result<Vec<ToolCall>, ModelError> {
    calls
        .into_iter()
        .map(|call| {
            let arguments = serde_json::from_str(&call.function.arguments)
                .map_err(|_| ModelError::invalid_response(body))?;
            Ok(ToolCall {
                id: call.id,
                name: call.function.name,
                arguments,
            })
        })
        .collect()
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
