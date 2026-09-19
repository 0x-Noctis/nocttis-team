mod model_gateway;

use std::{env, net::SocketAddr, time::Duration};

use anyhow::Context;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderValue, Method},
    routing::{get, post},
};
use serde::Serialize;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::info;

use crate::model_gateway::{OpenAiClient, ProbeResult};

#[derive(Clone)]
struct AppState {
    database: PgPool,
    model: OpenAiClient,
}

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

    let database_url = env::var("DATABASE_URL").context("DATABASE_URL is required")?;
    let database = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database_url)
        .await
        .context("failed to connect to PostgreSQL")?;
    sqlx::migrate!().run(&database).await?;

    let state = AppState {
        database,
        model: OpenAiClient::from_env()?,
    };

    let app = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/providers/primary/probe", post(probe_provider))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin("http://127.0.0.1:5173".parse::<HeaderValue>()?)
                .allow_methods([Method::GET, Method::POST]),
        );

    let address: SocketAddr = env::var("BIND_ADDRESS")
        .unwrap_or_else(|_| "127.0.0.1:7410".into())
        .parse()
        .context("invalid BIND_ADDRESS")?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    info!(%address, "server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn health(State(state): State<AppState>) -> Result<Json<Health>, AppError> {
    sqlx::query("SELECT 1").execute(&state.database).await?;
    Ok(Json(Health {
        status: "ok",
        database: "ok",
    }))
}

async fn probe_provider(State(state): State<AppState>) -> Result<Json<ProbeResult>, AppError> {
    Ok(Json(state.model.probe().await?))
}

struct AppError(anyhow::Error);

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(error: E) -> Self {
        Self(error.into())
    }
}

impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        tracing::error!(error = %self.0, "request failed");
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": {
                    "code": "INTERNAL_ERROR",
                    "message": self.0.to_string()
                }
            })),
        )
            .into_response()
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
