#[path = "../src/model/mod.rs"]
mod model;
mod openai {
    pub mod tools {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/model/openai/tools.rs"
        ));
    }
}

use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
    time::Duration,
};

use model::{
    FinishReason, Message, MessageRole, ModelErrorKind, ModelLimits, ModelRequest, ToolDefinition,
};
use openai::tools::{OpenAiToolsClient, ToolProbeResult};
use serde_json::json;

fn request() -> ModelRequest {
    ModelRequest {
        project_id: "project".to_owned(),
        task_id: "task".to_owned(),
        agent_run_id: "run".to_owned(),
        model_class: "coding".to_owned(),
        messages: vec![Message {
            role: MessageRole::User,
            content: "check weather".to_owned(),
            tool_call_id: None,
        }],
        tools: vec![ToolDefinition {
            name: "weather".to_owned(),
            description: "Gets weather".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"]
            }),
        }],
        limits: ModelLimits {
            max_input_tokens: 100,
            max_output_tokens: 20,
        },
    }
}

fn mock_server(body: &str) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = mpsc::channel();
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
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
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

fn client(base_url: &str) -> OpenAiToolsClient {
    OpenAiToolsClient::new(base_url, "", "test-model", Duration::from_secs(1)).unwrap()
}

#[tokio::test]
async fn normalizes_single_tool_call_and_definition() {
    let body = r#"{
        "choices":[{"message":{"content":null,"tool_calls":[
            {"id":"call-1","type":"function","function":{"name":"weather","arguments":"{\"city\":\"Jakarta\"}"}}
        ]},"finish_reason":"tool_calls"}]
    }"#;
    let (base_url, received, handle) = mock_server(body);

    let response = client(&base_url).complete(&request()).await.unwrap();
    let sent = received.recv().unwrap();
    handle.join().unwrap();

    assert_eq!(response.finish_reason, FinishReason::ToolCalls);
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].id, "call-1");
    assert_eq!(response.tool_calls[0].name, "weather");
    assert_eq!(response.tool_calls[0].arguments, json!({"city": "Jakarta"}));
    assert!(sent.contains(r#""type":"function""#));
    assert!(sent.contains(r#""name":"weather""#));
    assert!(sent.contains(r#""parameters""#));
}

#[tokio::test]
async fn normalizes_multiple_tool_calls_in_order() {
    let body = r#"{
        "choices":[{"message":{"content":null,"tool_calls":[
            {"id":"call-1","function":{"name":"weather","arguments":"{\"city\":\"Jakarta\"}"}},
            {"id":"call-2","function":{"name":"weather","arguments":"{\"city\":\"Bandung\"}"}}
        ]},"finish_reason":"tool_calls"}]
    }"#;
    let (base_url, _, handle) = mock_server(body);

    let response = client(&base_url).complete(&request()).await.unwrap();
    handle.join().unwrap();

    assert_eq!(response.tool_calls.len(), 2);
    assert_eq!(response.tool_calls[0].id, "call-1");
    assert_eq!(response.tool_calls[1].id, "call-2");
    assert_eq!(response.tool_calls[1].arguments, json!({"city": "Bandung"}));
}

#[tokio::test]
async fn malformed_tool_arguments_are_typed_invalid_response() {
    let body = r#"{
        "choices":[{"message":{"content":null,"tool_calls":[
            {"id":"call-1","function":{"name":"weather","arguments":"not-json-private"}}
        ]},"finish_reason":"tool_calls"}]
    }"#;
    let (base_url, _, handle) = mock_server(body);

    let error = client(&base_url).complete(&request()).await.unwrap_err();
    handle.join().unwrap();

    assert_eq!(error.kind(), ModelErrorKind::InvalidResponse);
    assert_eq!(error.to_string(), "invalid_response");
    assert!(!error.to_string().contains("not-json-private"));
}

#[tokio::test]
async fn provider_without_tools_returns_unsupported() {
    let body = r#"{
        "choices":[{"message":{"content":"I cannot call tools"},"finish_reason":"stop"}]
    }"#;
    let (base_url, _, handle) = mock_server(body);

    let result = client(&base_url).probe().await.unwrap();
    handle.join().unwrap();

    assert_eq!(result, ToolProbeResult::Unsupported);
}

#[tokio::test]
async fn probe_returns_call_without_executing_tool() {
    let body = r#"{
        "choices":[{"message":{"content":null,"tool_calls":[
            {"id":"probe-call","function":{"name":"noctis_capability_probe","arguments":"{\"enabled\":true}"}}
        ]},"finish_reason":"tool_calls"}]
    }"#;
    let (base_url, received, handle) = mock_server(body);

    let result = client(&base_url).probe().await.unwrap();
    let sent = received.recv().unwrap();
    handle.join().unwrap();

    let ToolProbeResult::Supported(call) = result else {
        panic!("expected supported tool call");
    };
    assert_eq!(call.name, "noctis_capability_probe");
    assert_eq!(call.arguments, json!({"enabled": true}));
    assert!(sent.contains("noctis_capability_probe"));
}

#[tokio::test]
async fn probe_rejects_wrong_payloads_as_unsupported() {
    let cases = [
        ("wrong_tool", r#"{\"enabled\":true}"#),
        ("noctis_capability_probe", r#"{\"enabled\":false}"#),
        ("noctis_capability_probe", "{}"),
        (
            "noctis_capability_probe",
            r#"{\"enabled\":true,\"extra\":true}"#,
        ),
    ];

    for (name, arguments) in cases {
        let body = format!(
            r#"{{
                "choices":[{{"message":{{"content":null,"tool_calls":[
                    {{"id":"probe-call","function":{{"name":"{name}","arguments":"{arguments}"}}}}
                ]}},"finish_reason":"tool_calls"}}]
            }}"#
        );
        let (base_url, _, handle) = mock_server(&body);

        let result = client(&base_url).probe().await.unwrap();
        handle.join().unwrap();

        assert_eq!(result, ToolProbeResult::Unsupported, "accepted {arguments}");
    }
}
