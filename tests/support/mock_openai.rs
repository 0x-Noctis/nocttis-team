use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

pub enum Scenario {
    NormalMeasured,
    NormalMissingUsage,
    ToolCall,
    AuthenticationFailure,
    RateLimit,
    Timeout,
    ContextOverflow,
    ProviderUnavailable,
    MalformedResponse,
    StreamingMeasured,
    FragmentedStreaming,
    DisconnectBeforeDone,
}

pub struct MockOpenAi {
    pub base_url: String,
    handle: thread::JoinHandle<String>,
}

impl MockOpenAi {
    pub fn start(scenario: Scenario) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            serve(&mut stream, scenario);
            request
        });
        Self {
            base_url: format!("http://{address}/v1"),
            handle,
        }
    }

    pub fn finish(self) -> String {
        self.handle.join().unwrap()
    }
}

fn serve(stream: &mut impl Write, scenario: Scenario) {
    match scenario {
        Scenario::NormalMeasured => response(
            stream,
            "200 OK",
            r#"{"choices":[{"message":{"content":"compatible"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12,"prompt_tokens_details":{"cached_tokens":3}}}"#,
        ),
        Scenario::NormalMissingUsage => response(
            stream,
            "200 OK",
            r#"{"choices":[{"message":{"content":"compatible"},"finish_reason":"stop"}]}"#,
        ),
        Scenario::ToolCall => response(
            stream,
            "200 OK",
            r#"{"choices":[{"message":{"content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"lookup","arguments":"{\"id\":7}"}}]},"finish_reason":"tool_calls"}]}"#,
        ),
        Scenario::AuthenticationFailure => response(
            stream,
            "401 Unauthorized",
            r#"{"error":{"code":"invalid_api_key","message":"sensitive-marker"}}"#,
        ),
        Scenario::RateLimit => response(
            stream,
            "429 Too Many Requests",
            r#"{"error":{"code":"rate_limit_exceeded","message":"sensitive-marker"}}"#,
        ),
        Scenario::Timeout => {
            thread::sleep(Duration::from_millis(100));
            response(stream, "200 OK", r#"{"choices":[]}"#);
        }
        Scenario::ContextOverflow => response(
            stream,
            "400 Bad Request",
            r#"{"error":{"code":"context_length_exceeded","message":"sensitive-marker"}}"#,
        ),
        Scenario::ProviderUnavailable => response(
            stream,
            "503 Service Unavailable",
            r#"{"error":{"code":"service_unavailable","message":"sensitive-marker"}}"#,
        ),
        Scenario::MalformedResponse => response(stream, "200 OK", "not-json-sensitive-marker"),
        Scenario::StreamingMeasured => chunked(
            stream,
            &[
                r#"data: {"choices":[{"delta":{"content":"stream"},"finish_reason":"stop"}]}

"#,
                r#"data: {"choices":[],"usage":{"prompt_tokens":8,"completion_tokens":2,"total_tokens":10,"prompt_tokens_details":{"cached_tokens":2}}}

"#,
                "data: [DONE]\n\n",
            ],
        ),
        Scenario::FragmentedStreaming => chunked(
            stream,
            &[
                "da",
                r#"ta: {"choices":[{"delta":{"content":"frag"},"finish_reason":null}]}

"#,
                r#"data: {"choices":[{"delta":{"content":"mented"},"finish_reason":"stop"}]}

"#,
                "data: [DONE]\n\n",
            ],
        ),
        Scenario::DisconnectBeforeDone => chunked(
            stream,
            &[
                r#"data: {"choices":[{"delta":{"content":"partial"},"finish_reason":null}]}

"#,
            ],
        ),
    }
}

fn response(stream: &mut impl Write, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn chunked(stream: &mut impl Write, fragments: &[&str]) {
    let _ = stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
    );
    for fragment in fragments {
        let _ = write!(stream, "{:X}\r\n{}\r\n", fragment.len(), fragment);
        let _ = stream.flush();
    }
    let _ = stream.write_all(b"0\r\n\r\n");
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
        if complete_request(&request) {
            break;
        }
    }
    String::from_utf8(request).unwrap()
}

fn complete_request(request: &[u8]) -> bool {
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
