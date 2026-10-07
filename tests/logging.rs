// M5 (log terstruktur): satu objek JSON per baris, request_id di setiap log selama request, tanpa data sensitif.
// Subscriber dipasang global sekali per proses uji (server dijalankan di thread lain), lalu baris difilter per request_id.
use std::{
    io::Write,
    sync::{Arc, Mutex, OnceLock},
};

use ai_team::{api, observability};
use axum::{Router, middleware, routing::get};
use reqwest::Client;
use serde_json::Value;
use tokio::net::TcpListener;
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Buffer {
    type Writer = Buffer;
    fn make_writer(&'a self) -> Buffer {
        self.clone()
    }
}

fn buffer() -> &'static Buffer {
    static BUFFER: OnceLock<Buffer> = OnceLock::new();
    BUFFER.get_or_init(|| {
        let buffer = Buffer::default();
        tracing::subscriber::set_global_default(observability::json_subscriber(buffer.clone()))
            .unwrap();
        buffer
    })
}

async fn handler() -> &'static str {
    tracing::info!("handler berjalan");
    "ok"
}

async fn base() -> String {
    buffer();
    let app = Router::new()
        .route("/ping", get(handler))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(observability::request_span)
                .on_response(DefaultOnResponse::new().level(tracing::Level::INFO)),
        )
        .layer(middleware::from_fn(api::request_id));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

/// Semua baris log sejauh ini sebagai JSON; setiap baris harus objek JSON valid.
fn lines() -> Vec<Value> {
    String::from_utf8(buffer().0.lock().unwrap().clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|_| panic!("bukan JSON: {line}")))
        .collect()
}

fn for_request(request_id: &str) -> Vec<Value> {
    lines()
        .into_iter()
        .filter(|line| line["span"]["request_id"] == request_id)
        .collect()
}

#[tokio::test]
async fn logs_are_json_with_request_id_from_a_valid_client_header() {
    let base = base().await;
    let id = "11111111-2222-4333-8444-555555555555";
    let response = Client::new()
        .get(format!("{base}/ping?api_key=SUPERSECRET"))
        .header("x-request-id", id)
        .header("authorization", "Bearer TOPSECRETTOKEN")
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["x-request-id"], id);

    let logs = for_request(id);
    let handler_line = logs
        .iter()
        .find(|line| line["fields"]["message"] == "handler berjalan")
        .expect("log handler harus membawa request_id");
    assert_eq!(handler_line["level"], "INFO");
    assert!(handler_line["timestamp"].is_string());
    assert_eq!(handler_line["span"]["path"], "/ping");
    assert_eq!(handler_line["span"]["method"], "GET");
    // Log akses selesai-request mencatat status.
    assert!(logs.iter().any(|line| {
        line["fields"]["message"] == "finished processing request"
            && line["fields"]["status"] == 200
    }));
    // Query string dan header tidak pernah masuk log.
    let raw = String::from_utf8(buffer().0.lock().unwrap().clone()).unwrap();
    assert!(!raw.contains("SUPERSECRET") && !raw.contains("TOPSECRETTOKEN"));
}

#[tokio::test]
async fn invalid_or_missing_client_request_id_is_replaced_with_a_fresh_uuid() {
    let base = base().await;
    for given in [
        None,
        Some("not-a-uuid"),
        Some("\"; DROP\n{\"x\":1}"),
        Some("11111111-2222-4333-8444-55555555555"),
    ] {
        let mut request = Client::new().get(format!("{base}/ping"));
        if let Some(value) = given {
            // Header dengan karakter kontrol tidak bisa dikirim sama sekali; yang bisa dikirim harus tetap diganti.
            if value.contains('\n') {
                continue;
            }
            request = request.header("x-request-id", value);
        }
        let response = request.send().await.unwrap();
        let id = response.headers()["x-request-id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert_ne!(Some(id.as_str()), given);
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        assert!(
            !for_request(&id).is_empty(),
            "log harus memakai ID yang sama dengan header respons"
        );
    }
}
