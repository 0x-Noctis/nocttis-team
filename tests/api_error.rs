#[path = "../src/api/error.rs"]
#[allow(dead_code)]
mod error;

use axum::{body::to_bytes, http::StatusCode, response::IntoResponse};
use error::{AppError, RequestId};
use serde_json::{Value, json};

#[tokio::test]
async fn stable_error_envelopes_cover_http_statuses() {
    let cases = [
        (
            AppError::bad_request(RequestId::new(), json!({"field": "name"})),
            StatusCode::BAD_REQUEST,
            "BAD_REQUEST",
        ),
        (
            AppError::not_found(RequestId::new(), Value::Null),
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
        ),
        (
            AppError::conflict(RequestId::new(), Value::Null),
            StatusCode::CONFLICT,
            "CONFLICT",
        ),
        (
            AppError::unprocessable(RequestId::new(), Value::Null),
            StatusCode::UNPROCESSABLE_ENTITY,
            "UNPROCESSABLE_ENTITY",
        ),
        (
            AppError::rate_limited(RequestId::new(), Value::Null),
            StatusCode::TOO_MANY_REQUESTS,
            "RATE_LIMITED",
        ),
        (
            AppError::internal(
                RequestId::new(),
                anyhow::anyhow!("database password leaked"),
            ),
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
        ),
    ];

    for (error, status, code) in cases {
        let response = error.into_response();
        assert_eq!(response.status(), status);
        let header_request_id = response.headers()["x-request-id"]
            .to_str()
            .unwrap()
            .to_owned();
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["error"]["code"], code);
        assert!(body["error"]["message"].is_string());
        assert!(body["error"].get("details").is_some());
        assert_eq!(body["error"]["request_id"], header_request_id);
        assert!(!body.to_string().contains("database password leaked"));
    }
}
