mod config;

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use axum::{Json, Router, extract::Extension, http::HeaderValue, middleware, routing::get};
use serde::Serialize;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::sync::{mpsc, watch};
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::config::Config;
use ai_team::api::tasks::StartState;
use ai_team::api::{self, AppError, RequestId, request_id};
use ai_team::orchestrator::scheduler::parallel::{ParallelConfig, ParallelScheduler};
use ai_team::orchestrator::{
    Orchestrator, OrchestratorConfig, OrchestratorRunner, repository_head,
};
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
    let orchestrator = Orchestrator::new(
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
    let orchestrator = Arc::new(orchestrator);
    let providers = ProviderRepository::new(database.clone());
    let parallel = ParallelConfig {
        slots: config.scheduler.max_parallel_agents,
        // Diisi setelah model terdaftar (lihat `run_parallel`).
        provider_id: String::new(),
        model_id: config.provider.model.clone(),
        retention_seconds: i64::try_from(config.git.retention_hours)
            .unwrap_or(i64::MAX / 3_600)
            .saturating_mul(3_600),
        lease_ttl_seconds: i64::try_from(config.scheduler.stale_after_seconds)
            .unwrap_or(i64::MAX)
            .max(
                i64::try_from(config.scheduler.heartbeat_seconds.saturating_mul(3))
                    .unwrap_or(i64::MAX),
            ),
        heartbeat_interval: Duration::from_secs(config.scheduler.heartbeat_seconds.max(1)),
        stale_after_seconds: i64::try_from(config.scheduler.stale_after_seconds)
            .unwrap_or(i64::MAX),
        idle_min: Duration::from_millis(200),
        idle_max: Duration::from_secs(2),
        shutdown_grace: Duration::from_secs(10),
        candidate_window: 50,
        recover_every: Duration::from_secs(config.scheduler.heartbeat_seconds.max(1) * 3),
    };
    let (stop, stopped) = watch::channel(false);
    let scheduler = tokio::spawn(run_parallel(
        database.clone(),
        providers,
        orchestrator.clone(),
        parallel,
        stopped,
    ));

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
    // `run` melayani attempt yang dimulai manual lewat API dan menyapu retensi worktree.
    tokio::spawn(orchestrator.run(wake_receiver));
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await;
    // Minta scheduler berhenti mengambil task baru, lalu tunggu slot yang berjalan (ada batas waktu di scheduler).
    let _ = stop.send(true);
    let _ = scheduler.await;
    Ok(served?)
}

/// Menunggu model terdaftar (provider/model dibuat lewat API, bukan saat startup), lalu menjalankan
/// `ParallelScheduler` sampai `stopped` bernilai true.
async fn run_parallel(
    database: PgPool,
    providers: ProviderRepository,
    orchestrator: Arc<Orchestrator>,
    mut config: ParallelConfig,
    mut stopped: watch::Receiver<bool>,
) {
    loop {
        match providers.get_model(&config.model_id).await {
            Ok(model) => {
                config.provider_id = model.provider_id.as_str().to_owned();
                break;
            }
            Err(_) => {
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_secs(1)) => {}
                    _ = stopped.changed() => return,
                }
            }
        }
    }
    let scheduler = match ParallelScheduler::new(
        database,
        OrchestratorRunner::new(orchestrator),
        config,
        repository_head,
    ) {
        Ok(scheduler) => scheduler,
        Err(error) => {
            tracing::error!(%error, "konfigurasi scheduler paralel tidak valid");
            return;
        }
    };
    let run = tokio::spawn(scheduler.clone().run());
    let _ = stopped.wait_for(|stop| *stop).await;
    scheduler.shutdown();
    let _ = run.await;
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
