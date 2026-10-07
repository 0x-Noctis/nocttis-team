pub mod artifacts;
pub mod contracts;
pub mod error;
pub mod events;
pub mod health;
pub mod lead;
pub mod operations;
pub mod projects;
pub mod providers;
pub mod runs;
pub mod tasks;

use axum::http::{HeaderName, HeaderValue, Method, header::CONTENT_TYPE};
use tower_http::cors::CorsLayer;

pub use error::{AppError, RequestId, request_id};

pub fn cors_layer(origin: HeaderValue) -> CorsLayer {
    cors_layer_for([origin])
}

/// CORS untuk daftar origin eksplisit (lihat `security::cors::parse_origins`); tidak pernah wildcard.
pub fn cors_layer_for(origins: impl IntoIterator<Item = HeaderValue>) -> CorsLayer {
    CorsLayer::new()
        .allow_origin(origins.into_iter().collect::<Vec<_>>())
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([
            CONTENT_TYPE,
            HeaderName::from_static("idempotency-key"),
            HeaderName::from_static("last-event-id"),
        ])
        .expose_headers([HeaderName::from_static("x-request-id")])
}
