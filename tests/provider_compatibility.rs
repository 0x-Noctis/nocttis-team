mod support {
    pub mod mock_openai;
}

use std::time::Duration;

use ai_team::{
    api,
    model::{
        FinishReason, Message, MessageRole, ModelErrorKind, ModelLimits, ModelRequest,
        ToolDefinition,
    },
    openai::{OpenAiChatClient, OpenAiStreamClient, OpenAiToolsClient},
};
use axum::middleware;
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
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

async fn start_api(pool: PgPool) -> String {
    let app = api::providers::router(pool)
        .fallback(api::error::not_found)
        .layer(middleware::from_fn(api::request_id));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}")
}

async fn mutate(
    client: &Client,
    method: Method,
    url: &str,
    key: &str,
    body: Option<&Value>,
) -> reqwest::Response {
    let mut request = client.request(method, url).header("Idempotency-Key", key);
    if let Some(body) = body {
        request = request.json(body);
    }
    request.send().await.unwrap()
}

async fn create_provider_model(
    client: &Client,
    api_url: &str,
    provider_id: &str,
    model_id: &str,
    base_url: &str,
    timeout_seconds: i64,
) {
    let provider = json!({
        "id": provider_id,
        "base_url": base_url,
        "api_key_env": "PATH",
        "request_timeout_seconds": timeout_seconds
    });
    let response = mutate(
        client,
        Method::POST,
        &format!("{api_url}/api/v1/providers"),
        &format!("create-provider-{provider_id}"),
        Some(&provider),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.text().await.unwrap();
    assert!(!body.contains("Authorization"));
    assert!(!body.contains("sensitive-marker"));

    let model = json!({
        "id": model_id,
        "provider_id": provider_id,
        "remote_name": "compat-model",
        "class": "coding",
        "context_window": 4096,
        "max_output_tokens": 128,
        "claimed_capabilities": {
            "chat": true,
            "streaming": true,
            "tools": true,
            "parallel_tools": false
        }
    });
    let response = mutate(
        client,
        Method::POST,
        &format!("{api_url}/api/v1/providers/{provider_id}/models"),
        &format!("create-model-{model_id}"),
        Some(&model),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[sqlx::test(migrations = "./migrations")]
async fn production_api_persists_probes_capabilities_and_replay(pool: PgPool) {
    let api_url = start_api(pool.clone()).await;
    let client = Client::new();
    let cases = [
        ("chat", Scenario::NormalMeasured),
        ("streaming", Scenario::StreamingMeasured),
        ("tools", Scenario::ToolProbe),
    ];

    for (kind, scenario) in cases {
        let mock = MockOpenAi::start(scenario);
        let provider_id = format!("provider-{kind}");
        let model_id = format!("model-{kind}");
        create_provider_model(
            &client,
            &api_url,
            &provider_id,
            &model_id,
            &mock.base_url,
            2,
        )
        .await;
        let probe_url = format!("{api_url}/api/v1/models/{model_id}/probes/{kind}");
        let key = format!("probe-{kind}");
        let first = mutate(&client, Method::POST, &probe_url, &key, None).await;
        assert_eq!(first.status(), StatusCode::OK);
        assert!(first.headers().contains_key("x-request-id"));
        let first_body: Value = first.json().await.unwrap();
        assert_eq!(first_body["result"]["verified"], "supported");

        let replay = mutate(&client, Method::POST, &probe_url, &key, None).await;
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(replay.json::<Value>().await.unwrap(), first_body);
        let request = mock.finish();
        assert!(!request.contains("sensitive-marker"));

        let probe_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM provider_probes WHERE provider_id = $1 AND model_id = $2",
        )
        .bind(&provider_id)
        .bind(&model_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(probe_count, 1);
        let verified: Value =
            sqlx::query_scalar("SELECT verified_capabilities FROM models WHERE id = $1")
                .bind(&model_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(verified[kind], "supported");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn production_api_normalizes_provider_failures(pool: PgPool) {
    let api_url = start_api(pool.clone()).await;
    let client = Client::new();
    let cases = [
        (
            "auth",
            Scenario::AuthenticationFailure,
            "authentication_failed",
            StatusCode::OK,
            2,
        ),
        (
            "rate",
            Scenario::RateLimit,
            "rate_limited",
            StatusCode::TOO_MANY_REQUESTS,
            2,
        ),
        (
            "timeout",
            Scenario::ApiTimeout,
            "timeout",
            StatusCode::OK,
            1,
        ),
        (
            "context",
            Scenario::ContextOverflow,
            "context_too_large",
            StatusCode::OK,
            2,
        ),
        (
            "unavailable",
            Scenario::ProviderUnavailable,
            "provider_unavailable",
            StatusCode::OK,
            2,
        ),
    ];

    for (name, scenario, error_code, expected_status, timeout_seconds) in cases {
        let mock = MockOpenAi::start(scenario);
        let provider_id = format!("provider-{name}");
        let model_id = format!("model-{name}");
        create_provider_model(
            &client,
            &api_url,
            &provider_id,
            &model_id,
            &mock.base_url,
            timeout_seconds,
        )
        .await;
        let response = mutate(
            &client,
            Method::POST,
            &format!("{api_url}/api/v1/models/{model_id}/probes/chat"),
            &format!("probe-{name}"),
            None,
        )
        .await;
        assert_eq!(response.status(), expected_status, "{name}");
        let request_id = response
            .headers()
            .get("x-request-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let body: Value = response.json().await.unwrap();
        let serialized = body.to_string();
        assert!(!serialized.contains("sensitive-marker"));
        assert!(!serialized.contains("invalid_api_key"));
        assert!(!serialized.contains("context_length_exceeded"));
        if expected_status == StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(body["error"]["code"], "RATE_LIMITED");
            assert_eq!(body["error"]["request_id"], request_id);
        } else {
            assert_eq!(body["result"]["status"], "failed");
            assert_eq!(body["result"]["error_code"], error_code);
            assert_eq!(body["result"]["verified"], "unsupported");
        }
        mock.finish();

        let stored: (String, String) = sqlx::query_as(
            "SELECT error_code, capability_status FROM provider_probes
             WHERE provider_id = $1 AND model_id = $2",
        )
        .bind(&provider_id)
        .bind(&model_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stored.0, error_code);
        assert_eq!(stored.1, "unsupported");
        let verified: Value =
            sqlx::query_scalar("SELECT verified_capabilities FROM models WHERE id = $1")
                .bind(&model_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(verified["chat"], "unsupported");
    }
}
