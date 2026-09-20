use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

use ai_team::{
    agent::{reviewer::ReviewerModel, worker::WorkerModel},
    model::{Message, MessageRole, ModelErrorKind, ModelLimits, ModelRequest, ToolDefinition},
    openai::OpenAiToolsClient,
};
use serde_json::json;

fn request(tools: bool) -> ModelRequest {
    ModelRequest {
        project_id: "project".into(),
        task_id: "task".into(),
        agent_run_id: "run".into(),
        model_class: "coding".into(),
        messages: vec![Message {
            role: MessageRole::User,
            content: "respond".into(),
            tool_call_id: None,
        }],
        tools: tools
            .then(|| ToolDefinition {
                name: "edit".into(),
                description: "edit file".into(),
                input_schema: json!({"type":"object"}),
            })
            .into_iter()
            .collect(),
        limits: ModelLimits {
            max_input_tokens: 100,
            max_output_tokens: 20,
        },
    }
}

fn server(status: &str, body: &str, delay: Duration) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let status = status.to_owned();
    let body = body.to_owned();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        while let Ok(read) = stream.read(&mut buffer) {
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            if request_complete(&bytes) {
                break;
            }
        }
        thread::sleep(delay);
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    });
    (format!("http://{address}/v1"), handle)
}

fn request_complete(request: &[u8]) -> bool {
    let Some(headers_end) = request.windows(4).position(|part| part == b"\r\n\r\n") else {
        return false;
    };
    let content_length = String::from_utf8_lossy(&request[..headers_end])
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or_default();
    request.len() >= headers_end + 4 + content_length
}

fn client(url: &str, secret: &str, timeout: Duration) -> OpenAiToolsClient {
    OpenAiToolsClient::new(url, secret, "model", timeout).unwrap()
}

#[tokio::test]
async fn worker_and_reviewer_use_production_async_adapter_losslessly() {
    let body = r#"{"choices":[{"message":{"content":"done","tool_calls":[{"id":"call-1","function":{"name":"edit","arguments":"{\"path\":\"src/lib.rs\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":9,"completion_tokens":4,"total_tokens":13,"prompt_tokens_details":{"cached_tokens":3}}}"#;
    let (url, handle) = server("200 OK", body, Duration::ZERO);
    let mut worker = client(&url, "worker-secret", Duration::from_secs(1));
    let response = WorkerModel::complete(&mut worker, &request(true))
        .await
        .unwrap();
    handle.join().unwrap();
    assert_eq!(response.usage.input_tokens, 9);
    assert_eq!(response.usage.cached_tokens, 3);
    assert_eq!(response.usage.output_tokens, 4);
    assert_eq!(response.usage.total_tokens, 13);
    assert!(!response.usage.estimated);
    assert_eq!(
        response.tool_calls[0].arguments,
        json!({"path":"src/lib.rs"})
    );

    let (url, handle) = server(
        "200 OK",
        r#"{"choices":[{"message":{"content":"{\"decision\":\"approved\",\"findings\":[]}","tool_calls":[]},"finish_reason":"stop"}]}"#,
        Duration::ZERO,
    );
    let mut reviewer = client(&url, "reviewer-secret", Duration::from_secs(1));
    let response = ReviewerModel::complete(&mut reviewer, &request(false))
        .await
        .unwrap();
    handle.join().unwrap();
    assert!(response.tool_calls.is_empty());
    assert!(response.usage.estimated);
}

#[tokio::test]
async fn production_adapter_errors_are_typed_and_redacted() {
    for (status, expected) in [
        ("401 Unauthorized", ModelErrorKind::AuthenticationFailed),
        ("429 Too Many Requests", ModelErrorKind::RateLimited),
    ] {
        let secret = "never-print-this-secret";
        let (url, handle) = server(status, secret, Duration::ZERO);
        let error = WorkerModel::complete(
            &mut client(&url, secret, Duration::from_secs(1)),
            &request(true),
        )
        .await
        .unwrap_err();
        handle.join().unwrap();
        assert_eq!(error.kind(), expected);
        assert!(!format!("{error}").contains(secret));
        assert!(!format!("{error:?}").contains(secret));
    }

    let (url, handle) = server("200 OK", "not-json", Duration::ZERO);
    let error = ReviewerModel::complete(
        &mut client(&url, "secret", Duration::from_secs(1)),
        &request(false),
    )
    .await
    .unwrap_err();
    handle.join().unwrap();
    assert_eq!(error.kind(), ModelErrorKind::InvalidResponse);
}

#[tokio::test]
async fn production_client_owns_timeout_without_blocking_runtime() {
    let (url, handle) = server("200 OK", "{}", Duration::from_millis(100));
    let error = WorkerModel::complete(
        &mut client(&url, "secret", Duration::from_millis(10)),
        &request(true),
    )
    .await
    .unwrap_err();
    handle.join().unwrap();
    assert_eq!(error.kind(), ModelErrorKind::Timeout);
}
