mod config;

use std::time::Duration;

use anyhow::Context;
use axum::{Json, Router, extract::Extension, http::HeaderValue, middleware, routing::get};
use serde::Serialize;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::sync::mpsc;
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::config::Config;
use ai_team::api::tasks::StartState;
use ai_team::api::{self, AppError, RequestId, request_id};
use ai_team::orchestrator::{Orchestrator, OrchestratorConfig, SequentialScheduler};
use ai_team::store::artifact::ArtifactStore;
use ai_team::store::{provider::ProviderRepository, task::TaskRepository};

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
    let artifact_store = ArtifactStore::new(
        &config.artifacts.root,
        config.artifacts.max_tool_output_bytes as u64,
    )?;
    let orchestrator_artifacts = ArtifactStore::new(
        &config.artifacts.root,
        config.artifacts.max_tool_output_bytes as u64,
    )?;
    let (wake, wake_receiver) = mpsc::channel(1);
    let mut orchestrator = Orchestrator::new(
        TaskRepository::new(database.clone()),
        ProviderRepository::new(database.clone()),
        orchestrator_artifacts,
        OrchestratorConfig {
            worktree_root: config.git.worktree_root.clone(),
            stale_after_seconds: config.scheduler.stale_after_seconds as i64,
            retention_lease_seconds: config.scheduler.heartbeat_seconds.max(1) as i64,
        },
    );
    // Pulihkan attempt yang ditinggalkan (termasuk lease dan reservasi budget-nya) sebelum recovery integrasi
    // milik orchestrator, supaya crash tepat setelah claim tidak menggagalkan startup.
    let recovered = ai_team::recovery::recover(
        &database,
        &ai_team::recovery::RecoveryConfig {
            worktree_root: config.git.worktree_root.clone(),
            stale_after_seconds: i64::try_from(config.scheduler.stale_after_seconds)
                .unwrap_or(i64::MAX),
        },
    )
    .await?;
    info!(?recovered, "startup recovery attempt selesai");
    orchestrator.startup_recovery().await?;
    let scheduler = SequentialScheduler::new(
        database.clone(),
        config.provider.model.clone(),
        i64::try_from(config.git.retention_hours)
            .unwrap_or(i64::MAX / 3_600)
            .saturating_mul(3_600),
    );

    let app = Router::new()
        .route("/api/v1/health", get(health))
        .merge(api::providers::router(database.clone()))
        .merge(api::projects::router_with_scheduler(
            database.clone(),
            Some(config.provider.model.clone()),
            config.scheduler.max_parallel_agents,
        ))
        .merge(api::tasks::router_with_start(
            database.clone(),
            artifact_store,
            Some(StartState {
                model_id: config.provider.model.clone(),
                retention_seconds: i64::try_from(config.git.retention_hours)
                    .unwrap_or(i64::MAX / 3_600)
                    .saturating_mul(3_600),
                wake,
            }),
        ))
        .fallback(api::error::not_found)
        .layer(Extension(database))
        .layer(TraceLayer::new_for_http())
        .layer(api::cors_layer(
            "http://127.0.0.1:5173".parse::<HeaderValue>()?,
        ))
        .layer(middleware::from_fn(request_id));

    let address = config.server.bind;
    let listener = tokio::net::TcpListener::bind(address).await?;
    info!(%address, "server listening");
    tokio::select! {
        result = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()) => result?,
        () = orchestrator.run_sequential(&scheduler, wake_receiver) => {}
    }
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
