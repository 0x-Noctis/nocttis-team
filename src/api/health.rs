//! Endpoint kesehatan dan metrik (M5-003).
//!
//! - `GET /api/v1/health`        : kompatibel dengan klien lama; 200 bila database menjawab.
//! - `GET /api/v1/health/live`   : proses hidup. Tidak menyentuh database maupun provider.
//! - `GET /api/v1/health/ready`  : 200 bila database dan migration sehat, selain itu 503. Worker/provider yang
//!   bermasalah hanya ditandai `degraded`; detail internal (pesan error, path) tidak pernah dikirim.
//! - `GET /api/v1/metrics`       : teks Prometheus; tetap menjawab 200 walau database mati (seri DB dilewati).

use axum::{
    Json, Router,
    extract::{Extension, State},
    http::{StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use sqlx::PgPool;

use super::{AppError, RequestId};
use crate::observability::{self, Component};

#[derive(Clone)]
struct HealthState {
    pool: PgPool,
    stale_after_seconds: i64,
}

pub fn router(pool: PgPool, stale_after_seconds: i64) -> Router {
    observability::mark_started();
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/health/live", get(live))
        .route("/api/v1/health/ready", get(ready))
        .route("/api/v1/metrics", get(metrics))
        .with_state(HealthState {
            pool,
            stale_after_seconds,
        })
}

async fn health(
    State(state): State<HealthState>,
    Extension(request_id): Extension<RequestId>,
) -> Result<Json<Value>, AppError> {
    sqlx::query("SELECT 1")
        .execute(&state.pool)
        .await
        .map_err(|error| AppError::internal(request_id, error))?;
    Ok(Json(json!({"status": "ok", "database": "ok"})))
}

async fn live() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn ready(State(state): State<HealthState>) -> Response {
    let readiness = observability::check(&state.pool, state.stale_after_seconds).await;
    let status = if readiness.ready() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = json!({
        "status": if readiness.ready() { "ready" } else { "unavailable" },
        "components": readiness,
    });
    (status, Json(body)).into_response()
}

async fn metrics(State(state): State<HealthState>) -> Response {
    let readiness = observability::check(&state.pool, state.stale_after_seconds).await;
    let snapshot = if readiness.database == Component::Ok {
        observability::snapshot(&state.pool, state.stale_after_seconds)
            .await
            .ok()
    } else {
        None
    };
    (
        [(CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        observability::render(&readiness, snapshot.as_ref()),
    )
        .into_response()
}
