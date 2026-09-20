pub mod artifacts;
pub mod contracts;
pub mod error;
pub mod events;
pub mod providers;
pub mod tasks;

use axum::http::{HeaderName, HeaderValue, Method, header::CONTENT_TYPE};
use tower_http::cors::CorsLayer;

pub use error::{AppError, RequestId, request_id};

pub fn cors_layer(origin: HeaderValue) -> CorsLayer {
    CorsLayer::new()
        .allow_origin(origin)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([
            CONTENT_TYPE,
            HeaderName::from_static("idempotency-key"),
            HeaderName::from_static("last-event-id"),
        ])
}
