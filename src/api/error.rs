use std::fmt;

use anyhow::Error;
use axum::{
    Json,
    extract::{Extension, Request},
    http::{HeaderName, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

#[derive(Clone, Copy, Debug)]
pub struct RequestId(Uuid);

impl RequestId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

pub async fn request_id(mut request: Request, next: Next) -> Response {
    let request_id = RequestId::new();
    request.extensions_mut().insert(request_id);
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        REQUEST_ID_HEADER,
        HeaderValue::from_str(&request_id.to_string()).expect("UUID is a valid header value"),
    );
    response
}

pub struct AppError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    details: Value,
    request_id: RequestId,
    cause: Option<Error>,
}

impl AppError {
    pub fn bad_request(request_id: RequestId, details: Value) -> Self {
        Self::public(
            StatusCode::BAD_REQUEST,
            "BAD_REQUEST",
            "Invalid request",
            details,
            request_id,
        )
    }

    pub fn not_found(request_id: RequestId, details: Value) -> Self {
        Self::public(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "Resource not found",
            details,
            request_id,
        )
    }

    pub fn method_not_allowed(request_id: RequestId, details: Value) -> Self {
        Self::public(
            StatusCode::METHOD_NOT_ALLOWED,
            "METHOD_NOT_ALLOWED",
            "Method not allowed",
            details,
            request_id,
        )
    }

    pub fn conflict(request_id: RequestId, details: Value) -> Self {
        Self::public(
            StatusCode::CONFLICT,
            "CONFLICT",
            "Resource conflict",
            details,
            request_id,
        )
    }

    pub fn unprocessable(request_id: RequestId, details: Value) -> Self {
        Self::public(
            StatusCode::UNPROCESSABLE_ENTITY,
            "UNPROCESSABLE_ENTITY",
            "Request could not be processed",
            details,
            request_id,
        )
    }

    pub fn rate_limited(request_id: RequestId, details: Value) -> Self {
        Self::public(
            StatusCode::TOO_MANY_REQUESTS,
            "RATE_LIMITED",
            "Rate limit exceeded",
            details,
            request_id,
        )
    }

    pub fn internal(request_id: RequestId, cause: impl Into<Error>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "INTERNAL_ERROR",
            message: "Internal server error",
            details: Value::Null,
            request_id,
            cause: Some(cause.into()),
        }
    }

    fn public(
        status: StatusCode,
        code: &'static str,
        message: &'static str,
        details: Value,
        request_id: RequestId,
    ) -> Self {
        Self {
            status,
            code,
            message,
            details,
            request_id,
            cause: None,
        }
    }
}

#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
    details: Value,
    request_id: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        if let Some(cause) = &self.cause {
            tracing::error!(request_id = %self.request_id, error = ?cause, "request failed");
        }
        let request_id = self.request_id.to_string();
        let mut response = (
            self.status,
            Json(ErrorEnvelope {
                error: ErrorBody {
                    code: self.code,
                    message: self.message,
                    details: self.details,
                    request_id: request_id.clone(),
                },
            }),
        )
            .into_response();
        response.headers_mut().insert(
            REQUEST_ID_HEADER,
            HeaderValue::from_str(&request_id).expect("UUID is a valid header value"),
        );
        response
    }
}

pub async fn not_found(Extension(request_id): Extension<RequestId>) -> AppError {
    AppError::not_found(request_id, Value::Null)
}
