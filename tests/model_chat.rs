#[path = "../src/model_gateway.rs"]
#[allow(dead_code)]
mod model_gateway;

use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
    time::Duration,
};

use model_gateway::{
    model::{FinishReason, Message, MessageRole, ModelErrorKind, ModelLimits, ModelRequest},
    openai::OpenAiChatClient,
};

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
            tool_calls: Vec::new(),
        }],
        tools: Vec::new(),
        limits: ModelLimits {
            max_input_tokens: 100,
            max_output_tokens: 20,
        },
    }
}

fn mock_server(
    status: &str,
    body: &str,
    delay: Duration,
) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = mpsc::channel();
    let status = status.to_owned();
    let body = body.to_owned();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    request.extend_from_slice(&buffer[..read]);
                    if complete_http_request(&request) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = sender.send(String::from_utf8(request).unwrap());
        thread::sleep(delay);
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    });
    (format!("http://{address}/v1"), receiver, handle)
}

fn complete_http_request(request: &[u8]) -> bool {
    let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    request.len() >= header_end + 4 + content_length
}

fn client(base_url: &str, timeout: Duration) -> OpenAiChatClient {
    OpenAiChatClient::new(base_url, "", "test-model", timeout).unwrap()
}

#[tokio::test]
async fn normalizes_success_content_usage_and_request_path() {
    let body = r#"{
        "choices":[{"message":{"content":"done"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":12,"completion_tokens":3,"total_tokens":15,
                 "prompt_tokens_details":{"cached_tokens":4}}
    }"#;
    let (base_url, received, handle) = mock_server("200 OK", body, Duration::ZERO);

    let response = client(&base_url, Duration::from_secs(1))
        .complete(&request())
        .await
        .unwrap();
    let sent = received.recv().unwrap();
    handle.join().unwrap();

    assert!(sent.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
    assert_eq!(response.content.as_deref(), Some("done"));
    assert_eq!(response.finish_reason, FinishReason::Stop);
    assert!(response.tool_calls.is_empty());
    assert_eq!(response.usage.input_tokens, 12);
    assert_eq!(response.usage.output_tokens, 3);
    assert_eq!(response.usage.cached_tokens, 4);
    assert_eq!(response.usage.total_tokens, 15);
    assert!(!response.usage.estimated);
}

#[tokio::test]
async fn keeps_v1_with_or_without_trailing_slash() {
    for suffix in ["", "/"] {
        let body = r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#;
        let (base_url, received, handle) = mock_server("200 OK", body, Duration::ZERO);

        client(&format!("{base_url}{suffix}"), Duration::from_secs(1))
            .complete(&request())
            .await
            .unwrap();
        let sent = received.recv().unwrap();
        handle.join().unwrap();

        assert!(sent.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
    }
}

#[tokio::test]
async fn normalizes_finish_reasons() {
    let cases = [
        ("stop", FinishReason::Stop),
        ("length", FinishReason::Length),
        ("tool_calls", FinishReason::ToolCalls),
        ("content_filter", FinishReason::ContentFilter),
        ("vendor_reason", FinishReason::Unknown),
    ];

    for (provider_reason, expected) in cases {
        let body = format!(
            r#"{{"choices":[{{"message":{{"content":"ok"}},"finish_reason":"{provider_reason}"}}]}}"#
        );
        let (base_url, _, handle) = mock_server("200 OK", &body, Duration::ZERO);
        let response = client(&base_url, Duration::from_secs(1))
            .complete(&request())
            .await
            .unwrap();
        handle.join().unwrap();
        assert_eq!(response.finish_reason, expected);
    }
}

#[tokio::test]
async fn marks_missing_usage_as_estimated_zero() {
    let body = r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#;
    let (base_url, _, handle) = mock_server("200 OK", body, Duration::ZERO);

    let response = client(&base_url, Duration::from_secs(1))
        .complete(&request())
        .await
        .unwrap();
    handle.join().unwrap();

    assert_eq!(response.usage.input_tokens, 0);
    assert_eq!(response.usage.output_tokens, 0);
    assert_eq!(response.usage.cached_tokens, 0);
    assert_eq!(response.usage.total_tokens, 0);
    assert!(response.usage.estimated);
}

#[tokio::test]
async fn maps_http_errors_without_exposing_provider_body() {
    let cases = [
        (
            "401 Unauthorized",
            r#"{"error":"auth-secret"}"#,
            ModelErrorKind::AuthenticationFailed,
        ),
        (
            "429 Too Many Requests",
            r#"{"error":"rate-secret"}"#,
            ModelErrorKind::RateLimited,
        ),
        (
            "500 Internal Server Error",
            r#"{"error":"server-secret"}"#,
            ModelErrorKind::ProviderUnavailable,
        ),
        (
            "400 Bad Request",
            r#"{"error":{"code":"context_length_exceeded","message":"context-secret"}}"#,
            ModelErrorKind::ContextTooLarge,
        ),
    ];

    for (status, body, expected) in cases {
        let (base_url, _, handle) = mock_server(status, body, Duration::ZERO);
        let error = client(&base_url, Duration::from_secs(1))
            .complete(&request())
            .await
            .unwrap_err();
        handle.join().unwrap();

        assert_eq!(error.kind(), expected);
        assert_eq!(error.to_string(), expected.to_string());
        assert!(!error.to_string().contains("secret"));
        assert!(!error.to_string().contains("error"));
    }
}

#[tokio::test]
async fn maps_timeout() {
    let body = r#"{"choices":[{"message":{"content":"late"},"finish_reason":"stop"}]}"#;
    let (base_url, _, handle) = mock_server("200 OK", body, Duration::from_millis(100));

    let error = client(&base_url, Duration::from_millis(10))
        .complete(&request())
        .await
        .unwrap_err();
    handle.join().unwrap();

    assert_eq!(error.kind(), ModelErrorKind::Timeout);
    assert_eq!(error.to_string(), "timeout");
}

#[tokio::test]
async fn rejects_malformed_json_and_missing_choices() {
    for body in ["not-json-secret", r#"{"choices":[]}"#] {
        let (base_url, _, handle) = mock_server("200 OK", body, Duration::ZERO);
        let error = client(&base_url, Duration::from_secs(1))
            .complete(&request())
            .await
            .unwrap_err();
        handle.join().unwrap();

        assert_eq!(error.kind(), ModelErrorKind::InvalidResponse);
        assert_eq!(error.to_string(), "invalid_response");
        assert!(!error.to_string().contains(body));
    }
}
