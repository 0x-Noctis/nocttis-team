mod support {
    pub mod mock_openai;
}

use std::time::Duration;

use ai_team::{
    model::{
        FinishReason, Message, MessageRole, ModelErrorKind, ModelLimits, ModelRequest,
        ToolDefinition,
    },
    openai::{OpenAiChatClient, OpenAiStreamClient, OpenAiToolsClient},
};
use serde_json::json;
use support::mock_openai::{MockOpenAi, Scenario};

fn request() -> ModelRequest {
    ModelRequest {
        project_id: "project".to_owned(),
        task_id: "task".to_owned(),
        agent_run_id: "run".to_owned(),
        model_class: "coding".to_owned(),
        messages: vec![Message {
            role: MessageRole::User,
            content: "hello".to_owned(),
            tool_call_id: None,
        }],
        tools: Vec::new(),
        limits: ModelLimits {
            max_input_tokens: 100,
            max_output_tokens: 20,
        },
    }
}

fn chat(base_url: &str, timeout: Duration) -> OpenAiChatClient {
    OpenAiChatClient::new(base_url, "", "compat-model", timeout).unwrap()
}

#[tokio::test]
async fn normal_completion_normalizes_measured_and_missing_usage() {
    let measured = MockOpenAi::start(Scenario::NormalMeasured);
    let response = chat(&measured.base_url, Duration::from_secs(1))
        .complete(&request())
        .await
        .unwrap();
    let sent = measured.finish();

    assert!(sent.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
    assert_eq!(response.content.as_deref(), Some("compatible"));
    assert_eq!(response.finish_reason, FinishReason::Stop);
    assert_eq!(response.usage.cached_tokens, 3);
    assert!(!response.usage.estimated);

    let missing = MockOpenAi::start(Scenario::NormalMissingUsage);
    let response = chat(&missing.base_url, Duration::from_secs(1))
        .complete(&request())
        .await
        .unwrap();
    missing.finish();

    assert_eq!(response.usage.total_tokens, 0);
    assert!(response.usage.estimated);
}

#[tokio::test]
async fn streaming_handles_usage_fragmentation_and_disconnect() {
    let measured = MockOpenAi::start(Scenario::StreamingMeasured);
    let response = OpenAiStreamClient::new(
        &measured.base_url,
        "",
        "compat-model",
        Duration::from_secs(1),
    )
    .unwrap()
    .complete(&request())
    .await
    .unwrap();
    measured.finish();

    assert_eq!(response.content.as_deref(), Some("stream"));
    assert_eq!(response.usage.cached_tokens, 2);
    assert!(!response.usage.estimated);

    let fragmented = MockOpenAi::start(Scenario::FragmentedStreaming);
    let response = OpenAiStreamClient::new(
        &fragmented.base_url,
        "",
        "compat-model",
        Duration::from_secs(1),
    )
    .unwrap()
    .complete(&request())
    .await
    .unwrap();
    fragmented.finish();
    assert_eq!(response.content.as_deref(), Some("fragmented"));

    let disconnected = MockOpenAi::start(Scenario::DisconnectBeforeDone);
    let error = OpenAiStreamClient::new(
        &disconnected.base_url,
        "",
        "compat-model",
        Duration::from_secs(1),
    )
    .unwrap()
    .complete(&request())
    .await
    .unwrap_err();
    disconnected.finish();
    assert_eq!(error.kind(), ModelErrorKind::ProviderUnavailable);
}

#[tokio::test]
async fn tool_call_is_normalized_without_execution() {
    let server = MockOpenAi::start(Scenario::ToolCall);
    let mut tool_request = request();
    tool_request.tools.push(ToolDefinition {
        name: "lookup".to_owned(),
        description: "Looks up one record".to_owned(),
        input_schema: json!({"type": "object"}),
    });

    let response =
        OpenAiToolsClient::new(&server.base_url, "", "compat-model", Duration::from_secs(1))
            .unwrap()
            .complete(&tool_request)
            .await
            .unwrap();
    server.finish();

    assert_eq!(response.finish_reason, FinishReason::ToolCalls);
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].name, "lookup");
    assert_eq!(response.tool_calls[0].arguments, json!({"id": 7}));
}

#[tokio::test]
async fn maps_http_failures_without_exposing_provider_data() {
    let cases = [
        (
            Scenario::AuthenticationFailure,
            ModelErrorKind::AuthenticationFailed,
        ),
        (Scenario::RateLimit, ModelErrorKind::RateLimited),
        (Scenario::ContextOverflow, ModelErrorKind::ContextTooLarge),
        (
            Scenario::ProviderUnavailable,
            ModelErrorKind::ProviderUnavailable,
        ),
        (Scenario::MalformedResponse, ModelErrorKind::InvalidResponse),
    ];

    for (scenario, expected) in cases {
        let server = MockOpenAi::start(scenario);
        let error = chat(&server.base_url, Duration::from_secs(1))
            .complete(&request())
            .await
            .unwrap_err();
        server.finish();

        assert_eq!(error.kind(), expected);
        assert_eq!(error.to_string(), expected.to_string());
        assert!(!error.to_string().contains("sensitive-marker"));
        assert!(!error.to_string().contains("invalid_api_key"));
        assert!(!error.to_string().contains("context_length_exceeded"));
    }
}

#[tokio::test]
async fn maps_timeout() {
    let server = MockOpenAi::start(Scenario::Timeout);
    let error = chat(&server.base_url, Duration::from_millis(10))
        .complete(&request())
        .await
        .unwrap_err();
    server.finish();

    assert_eq!(error.kind(), ModelErrorKind::Timeout);
    assert_eq!(error.to_string(), "timeout");
}
