#[path = "../src/model_gateway.rs"]
#[allow(dead_code)]
mod model_gateway;

use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

use model_gateway::{
    model::{FinishReason, Message, MessageRole, ModelErrorKind, ModelLimits, ModelRequest},
    openai::OpenAiStreamClient,
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
        }],
        tools: Vec::new(),
        limits: ModelLimits {
            max_input_tokens: 100,
            max_output_tokens: 20,
        },
    }
}

fn stream_server(fragments: Vec<&str>) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let fragments = fragments.into_iter().map(str::to_owned).collect::<Vec<_>>();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request(&mut stream);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        for fragment in fragments {
            write!(stream, "{:X}\r\n{}\r\n", fragment.len(), fragment).unwrap();
            stream.flush().unwrap();
            thread::sleep(Duration::from_millis(2));
        }
        stream.write_all(b"0\r\n\r\n").unwrap();
        request
    });
    (format!("http://{address}/v1"), handle)
}

fn response_server(status: &str, body: &str) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let status = status.to_owned();
    let body = body.to_owned();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request(&mut stream);
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    (format!("http://{address}/v1"), handle)
}

fn read_request(stream: &mut impl Read) -> String {
    let mut request = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let read = stream.read(&mut buffer).unwrap();
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        if complete_http_request(&request) {
            break;
        }
    }
    String::from_utf8(request).unwrap()
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

fn client(base_url: &str) -> OpenAiStreamClient {
    OpenAiStreamClient::new(base_url, "", "test-model", Duration::from_secs(1)).unwrap()
}

#[tokio::test]
async fn aggregates_fragmented_content_finish_reason_and_usage() {
    let fragments = vec![
        "da",
        "ta: {\"choices\":[{\"delta\":{\"content\":\"hel\"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":3,\"total_tokens\":15,\"prompt_tokens_details\":{\"cached_tokens\":4}}}\n\n",
        "data: [DONE]\n\n",
    ];
    let (base_url, handle) = stream_server(fragments);

    let response = client(&base_url).complete(&request()).await.unwrap();
    let sent = handle.join().unwrap();

    assert!(sent.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
    assert!(sent.contains("\"stream\":true"));
    assert_eq!(response.content.as_deref(), Some("hello"));
    assert_eq!(response.finish_reason, FinishReason::Stop);
    assert!(response.tool_calls.is_empty());
    assert_eq!(response.usage.input_tokens, 12);
    assert_eq!(response.usage.output_tokens, 3);
    assert_eq!(response.usage.cached_tokens, 4);
    assert_eq!(response.usage.total_tokens, 15);
    assert!(!response.usage.estimated);
}

#[tokio::test]
async fn accepts_empty_delta_and_done_with_estimated_usage() {
    let (base_url, handle) = stream_server(vec![
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":null}]}\n\n",
        "data: [DONE]\n\n",
    ]);

    let response = client(&base_url).complete(&request()).await.unwrap();
    handle.join().unwrap();

    assert_eq!(response.content, None);
    assert_eq!(response.finish_reason, FinishReason::Unknown);
    assert_eq!(response.usage.total_tokens, 0);
    assert!(response.usage.estimated);
}

#[tokio::test]
async fn rejects_disconnect_before_done() {
    let (base_url, handle) = stream_server(vec![
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n",
    ]);

    let error = client(&base_url).complete(&request()).await.unwrap_err();
    handle.join().unwrap();

    assert_eq!(error.kind(), ModelErrorKind::ProviderUnavailable);
}

#[tokio::test]
async fn rejects_provider_error_event_without_exposing_body() {
    let (base_url, handle) = stream_server(vec![
        "event: error\ndata: {\"error\":{\"message\":\"provider-secret\"}}\n\n",
    ]);

    let error = client(&base_url).complete(&request()).await.unwrap_err();
    handle.join().unwrap();

    assert_eq!(error.kind(), ModelErrorKind::InvalidResponse);
    assert_eq!(error.to_string(), "invalid_response");
    assert!(!error.to_string().contains("provider-secret"));
}

#[tokio::test]
async fn maps_http_error_before_streaming() {
    let (base_url, handle) = response_server(
        "429 Too Many Requests",
        r#"{"error":{"message":"provider-secret"}}"#,
    );

    let error = client(&base_url).complete(&request()).await.unwrap_err();
    handle.join().unwrap();

    assert_eq!(error.kind(), ModelErrorKind::RateLimited);
    assert_eq!(error.to_string(), "rate_limited");
}
