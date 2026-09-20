pub mod api;
mod config;
pub mod domain;
pub mod store;

use std::time::Duration;

use anyhow::Context;
use axum::{
    Json, Router,
    extract::Extension,
    http::{HeaderValue, Method},
    middleware,
    routing::get,
};
use serde::Serialize;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::info;

use crate::api::{AppError, RequestId, request_id};
use crate::config::Config;

#[derive(Serialize)]
struct Health {
    status: &'static str,
    database: &'static str,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let (config, secrets) = Config::load()?;
    let database = PgPoolOptions::new()
        .max_connections(config.database.max_connections)
        .acquire_timeout(Duration::from_secs(config.database.acquire_timeout_seconds))
        .connect(&secrets.database_url)
        .await
        .context("failed to connect to PostgreSQL")?;
    sqlx::migrate!().run(&database).await?;

    let app = Router::new()
        .route("/api/v1/health", get(health))
        .merge(api::providers::router(database.clone()))
        .fallback(api::error::not_found)
        .layer(Extension(database))
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin("http://127.0.0.1:5173".parse::<HeaderValue>()?)
                .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE]),
        )
        .layer(middleware::from_fn(request_id));

    let address = config.server.bind;
    let listener = tokio::net::TcpListener::bind(address).await?;
    info!(%address, "server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn health(
    Extension(database): Extension<PgPool>,
    Extension(request_id): Extension<RequestId>,
) -> Result<Json<Health>, AppError> {
    sqlx::query("SELECT 1")
        .execute(&database)
        .await
        .map_err(|error| AppError::internal(request_id, error))?;
    Ok(Json(Health {
        status: "ok",
        database: "ok",
    }))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
