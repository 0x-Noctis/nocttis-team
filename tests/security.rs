// M5-001: audit keamanan. Traversal/symlink, jalur proses, dan network sudah diuji mendalam di tool_policy.rs,
// git_worktree.rs, dan process_runner.rs (lihat docs/security.md); berkas ini menambah redaksi rahasia, kebocoran
// ke provider, batas payload, CORS, dan allowlist perintah.
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
    time::Duration,
};

use ai_team::{
    api,
    model::{Message, MessageRole, ModelLimits, ModelRequest},
    openai::tools::OpenAiToolsClient,
    runner::{
        container::ContainerLimits,
        process::{ProcessError, ProcessRunner, VerificationCommand},
    },
    security::{
        cors::{parse_origin, parse_origins},
        limits::{MAX_BODY_BYTES, body_limit},
        redact::{REDACTED, redact, redact_bytes, register_secret},
    },
};
use axum::{Json, Router, routing::post};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use tokio::net::TcpListener as AsyncListener;

const PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASC\nabcdef==\n-----END PRIVATE KEY-----";

#[test]
fn redaction_removes_every_known_secret_shape() {
    let cases = [
        (
            "OPENAI_API_KEY=sk-abcdefghijklmnopqrstuvwxyz012345",
            "sk-abcdefghijkl",
        ),
        (
            "Authorization: Bearer abcdef0123456789xyz",
            "abcdef0123456789xyz",
        ),
        (
            r#"{"password": "hunter2hunter2", "user": "ana"}"#,
            "hunter2hunter2",
        ),
        (
            concat!("token: gh", "p_abcdefghijklmnopqrstuvwxyz0123456789"),
            concat!("gh", "p_abcdefghijkl"),
        ),
        (concat!("aws AKI", "AABCDEFGHIJKLMNOP end"), concat!("AKI", "AABCDEFGHIJKLMNOP")),
        (concat!("slack xo", "xb-1234567890-abcdefghijklmnop"), concat!("xo", "xb-1234567890")),
        (
            "jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijklmnop",
            "eyJhbGciOiJIUzI1NiJ9",
        ),
        ("db_secret='correct-horse-battery'", "correct-horse-battery"),
        (
            &format!("before\n{PEM}\nafter"),
            "MIIEvQIBADANBgkqhkiG9w0BAQEFAASC",
        ),
    ];
    for (input, secret) in cases {
        let output = redact(input);
        assert!(!output.contains(secret), "bocor: {input} -> {output}");
        assert!(
            output.contains(REDACTED) || output.contains("[REDACTED PRIVATE KEY]"),
            "{output}"
        );
    }
    // Teks di sekitar rahasia dipertahankan.
    let input = format!("before\n{PEM}\nafter");
    let output = redact(&input);
    assert!(
        output.starts_with("before\n") && output.ends_with("\nafter"),
        "{output}"
    );
    assert!(
        redact(r#"{"password": "hunter2hunter2", "user": "ana"}"#).contains(r#""user": "ana""#)
    );
}

#[test]
fn redaction_leaves_ordinary_text_and_identifiers_alone() {
    let untouched = [
        "commit 3f786850e387550fdab836ed7e6dc881de23001b",
        "max_tokens: 1000, token_count=5",
        "the password field is required",
        "fn main() { println!(\"hello\"); }",
        "key=value and secret=short",
        "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----",
    ];
    for input in untouched {
        assert_eq!(redact(input), input, "false positive: {input}");
    }
    // Idempoten: meredaksi hasil redaksi tidak mengubah apa pun.
    let once = redact("api_key=sk-abcdefghijklmnopqrstuvwxyz012345").into_owned();
    assert_eq!(redact(&once), once);
    // Data biner (bukan UTF-8) tidak disentuh.
    assert_eq!(
        redact_bytes(&[0xff, 0xfe, b'x']).as_ref(),
        &[0xff, 0xfe, b'x']
    );
    assert_eq!(
        redact_bytes(b"password=abcdefghijk").as_ref(),
        b"password=[REDACTED]"
    );
}

#[test]
fn registered_secret_values_are_redacted_wherever_they_appear() {
    register_secret("unit-test-registered-value-42");
    register_secret("short"); // terlalu pendek: diabaikan
    let output = redact("log line unit-test-registered-value-42 and short words");
    assert_eq!(output, format!("log line {REDACTED} and short words"));
}

fn mock_provider() -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let (mut request, mut buffer) = (Vec::new(), [0; 8192]);
        while let Ok(read) = stream.read(&mut buffer) {
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")?
                            .trim()
                            .parse::<usize>()
                            .ok()
                    })
                    .unwrap_or(0);
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        let _ = sender.send(String::from_utf8_lossy(&request).into_owned());
        let body = r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    (format!("http://{address}/v1"), receiver)
}

#[tokio::test]
async fn model_client_never_sends_secrets_to_the_provider() {
    let (base_url, received) = mock_provider();
    let client = OpenAiToolsClient::new(
        &base_url,
        "provider-key-should-never-leak-1234",
        "fixture",
        Duration::from_secs(5),
    )
    .unwrap();
    let content = format!(
        "context file:\nAPI_KEY=sk-abcdefghijklmnopqrstuvwxyz012345\n{PEM}\nechoed key provider-key-should-never-leak-1234\nnormal text"
    );
    let request = ModelRequest {
        project_id: "p".into(),
        task_id: "t".into(),
        agent_run_id: "r".into(),
        model_class: "coding".into(),
        messages: vec![Message {
            role: MessageRole::User,
            content,
            tool_call_id: None,
        }],
        tools: Vec::new(),
        limits: ModelLimits {
            max_input_tokens: 100,
            max_output_tokens: 10,
        },
    };
    client.complete(&request).await.unwrap();

    let raw = received.recv_timeout(Duration::from_secs(2)).unwrap();
    let body = raw.split("\r\n\r\n").nth(1).unwrap();
    for secret in [
        "sk-abcdefghijklmnopqrstuvwxyz012345",
        "MIIEvQIBADANBgkqhkiG9w0BAQEFAASC",
        "provider-key-should-never-leak-1234",
    ] {
        assert!(!body.contains(secret), "bocor ke provider: {secret}");
    }
    assert!(body.contains("normal text") && body.contains("REDACTED"));
    // Kunci hanya boleh ada di header Authorization.
    assert!(
        raw.to_ascii_lowercase()
            .contains("authorization: bearer provider-key-should-never-leak-1234")
    );
}

async fn echo(Json(value): Json<Value>) -> Json<Value> {
    Json(json!({"bytes": value.to_string().len()}))
}

#[tokio::test]
async fn oversized_payload_is_rejected_by_the_global_body_limit() {
    let app = Router::new().route("/echo", post(echo)).layer(body_limit());
    let listener = AsyncListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/echo", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = Client::new();

    let small = client
        .post(&url)
        .json(&json!({"v": "x".repeat(1024)}))
        .send()
        .await
        .unwrap();
    assert_eq!(small.status(), StatusCode::OK);
    let big = json!({"v": "x".repeat(MAX_BODY_BYTES + 1)});
    let response = client.post(&url).json(&big).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[test]
fn cors_origins_must_be_explicit() {
    for ok in [
        "http://127.0.0.1:5173",
        "https://app.example.com",
        "http://localhost",
    ] {
        assert!(parse_origin(ok).is_ok(), "{ok}");
    }
    for bad in [
        "*",
        "http://*",
        "https://*.example.com",
        "ftp://example.com",
        "example.com",
        "http://",
        "http://example.com/",
        "http://example.com/path",
        "http://user@example.com",
        "http://exa mple.com",
        "http://example.com?x=1",
    ] {
        assert!(parse_origin(bad).is_err(), "{bad} harus ditolak");
    }
    assert!(parse_origins(&[]).is_err());
    assert!(parse_origins(&vec!["http://a.test".to_owned(); 9]).is_err());
}

#[tokio::test]
async fn cors_headers_follow_the_configured_origins_only() {
    let origins = parse_origins(&[
        "http://127.0.0.1:5173".to_owned(),
        "https://ui.example.com".to_owned(),
    ])
    .unwrap();
    let app = Router::new()
        .route("/ping", axum::routing::get(|| async { "ok" }))
        .layer(api::cors_layer_for(origins));
    let listener = AsyncListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/ping", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = Client::new();
    let allow = |origin: &'static str| {
        let (client, url) = (client.clone(), url.clone());
        async move {
            client
                .get(url)
                .header("origin", origin)
                .send()
                .await
                .unwrap()
                .headers()
                .get("access-control-allow-origin")
                .map(|value| value.to_str().unwrap().to_owned())
        }
    };
    assert_eq!(
        allow("http://127.0.0.1:5173").await.as_deref(),
        Some("http://127.0.0.1:5173")
    );
    assert_eq!(
        allow("https://ui.example.com").await.as_deref(),
        Some("https://ui.example.com")
    );
    assert_eq!(allow("https://evil.example").await, None);
    assert_eq!(allow("null").await, None);
}

#[test]
fn command_allowlist_rejects_shells_paths_and_shell_operators() {
    let dir = std::env::temp_dir();
    let build = |executable: &str, arguments: &[&str], environment: &[&str]| {
        ProcessRunner::new(
            &dir,
            "rust:1",
            vec![VerificationCommand {
                class: "verify".into(),
                executable: executable.into(),
                arguments: arguments.iter().map(|a| (*a).to_owned()).collect(),
            }],
            environment.iter().map(|e| (*e).to_owned()).collect(),
            Duration::from_secs(1),
            1024,
            ContainerLimits {
                cpu_count: "1".into(),
                memory_bytes: 1 << 28,
                process_limit: 64,
            },
        )
        .map(|_| ())
    };
    assert!(build("cargo", &["test", "--lib"], &["RUST_LOG"]).is_ok());
    for shell in ["sh", "bash", "dash", "zsh"] {
        assert_eq!(
            build(shell, &["-c", "id"], &[]),
            Err(ProcessError::InvalidCommand),
            "{shell}"
        );
    }
    for executable in ["/bin/ls", "../ls", "-rf", "a\\b", "ls\n"] {
        assert_eq!(
            build(executable, &[], &[]),
            Err(ProcessError::InvalidCommand),
            "{executable:?}"
        );
    }
    for argument in [
        "a; rm -rf /",
        "a | nc host 1",
        "a && id",
        "$(id)",
        "`id`",
        "a > /etc/x",
        "a < /etc/passwd",
        "a\nb",
    ] {
        assert_eq!(
            build("echo", &[argument], &[]),
            Err(ProcessError::InvalidCommand),
            "{argument:?}"
        );
    }
    for key in ["", "1BAD", "A=B", "A B", "A-B", "$HOME", "A\n"] {
        assert_eq!(
            build("echo", &[], &[key]),
            Err(ProcessError::EnvironmentDenied),
            "{key:?}"
        );
    }
}
