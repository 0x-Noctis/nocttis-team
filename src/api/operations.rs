//! Ringkasan operasional untuk halaman Operations/Settings (M5-008).
//!
//! - `GET /api/v1/operations`        : kesehatan komponen, hitungan task/attempt, pemakaian token (dengan penanda estimasi),
//!   kesiapan provider, dan status retensi. Hanya membaca; tidak pernah memuat nilai secret.
//! - `GET /api/v1/operations/config` : konfigurasi non-secret yang diberikan `main` saat start.
//!
//! Bila database tidak terjangkau endpoint tetap menjawab 200 dengan komponen `down` dan bagian data `null`, supaya UI dapat
//! menampilkan penyebabnya alih-alih halaman error kosong.

use std::{collections::BTreeMap, env, sync::Arc};

use axum::{Json, Router, extract::State, routing::get};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};

use crate::observability::{self, ATTEMPT_STATUSES, Component, TASK_STATUSES};

#[derive(Clone)]
struct OperationsState {
    pool: PgPool,
    stale_after_seconds: i64,
    config: Arc<Value>,
}

pub fn router(pool: PgPool, stale_after_seconds: i64, config: Value) -> Router {
    Router::new()
        .route("/api/v1/operations", get(summary))
        .route("/api/v1/operations/config", get(configuration))
        .with_state(OperationsState {
            pool,
            stale_after_seconds,
            config: Arc::new(config),
        })
}

async fn configuration(State(state): State<OperationsState>) -> Json<Value> {
    Json((*state.config).clone())
}

async fn summary(State(state): State<OperationsState>) -> Json<Value> {
    let readiness = observability::check(&state.pool, state.stale_after_seconds).await;
    let mut body = json!({
        "ready": readiness.ready(),
        "components": readiness,
        "tasks": null, "attempts": null, "stale_attempts": null,
        "usage": null, "providers": null, "retention": null,
    });
    if readiness.database != Component::Ok {
        return Json(body);
    }
    // Kegagalan satu bagian tidak boleh menyembunyikan bagian lain: tiap bagian opsional.
    if let Ok(snapshot) = observability::snapshot(&state.pool, state.stale_after_seconds).await {
        let zero_filled = |names: &[&str],
                           counts: &std::collections::HashMap<String, i64>|
         -> BTreeMap<String, i64> {
            names
                .iter()
                .map(|name| ((*name).to_owned(), counts.get(*name).copied().unwrap_or(0)))
                .collect()
        };
        body["tasks"] = json!(zero_filled(&TASK_STATUSES, &snapshot.tasks));
        body["attempts"] = json!(zero_filled(&ATTEMPT_STATUSES, &snapshot.attempts));
        body["stale_attempts"] = json!(snapshot.stale_attempts);
        body["usage"] = json!({
            "input_tokens": snapshot.input_tokens,
            "output_tokens": snapshot.output_tokens,
            // Bagian total yang berasal dari estimasi (bukan angka provider); UI wajib melabelinya.
            "estimated_tokens": snapshot.estimated_tokens,
        });
    }
    if let Ok(cost) = cost(&state.pool).await {
        body["usage"]["cost"] = cost;
    }
    body["providers"] = providers(&state.pool).await.unwrap_or(Value::Null);
    body["retention"] = retention(&state.pool).await.unwrap_or(Value::Null);
    Json(body)
}

/// Biaya hanya tercatat bila provider/pencatat mengisi `cost_micros`; selain itu dinyatakan tidak dilacak.
async fn cost(pool: &PgPool) -> Result<Value, sqlx::Error> {
    let row = sqlx::query(
        "SELECT count(*) AS rows, count(cost_micros) AS priced, COALESCE(sum(cost_micros),0)::bigint AS micros FROM model_usage",
    )
    .fetch_one(pool)
    .await?;
    let (rows, priced): (i64, i64) = (row.get("rows"), row.get("priced"));
    Ok(json!({
        "tracked": priced > 0,
        "micros": row.get::<i64, _>("micros"),
        "priced_rows": priced,
        "total_rows": rows,
    }))
}

/// Provider dan model beserta kesiapannya. `secret_configured` hanya bilang variabel environment ada, bukan nilainya.
async fn providers(pool: &PgPool) -> Result<Value, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT p.id AS provider_id,p.api_key_env,p.base_url,m.id AS model_id,m.class,
                COALESCE(m.verified_capabilities->>'tools','unknown') AS tools
         FROM providers p LEFT JOIN models m ON m.provider_id=p.id ORDER BY p.id,m.id",
    )
    .fetch_all(pool)
    .await?;
    let mut grouped: BTreeMap<String, Value> = BTreeMap::new();
    for row in rows {
        let id: String = row.get("provider_id");
        let entry = grouped.entry(id.clone()).or_insert_with(|| {
            let base_url: String = row.get("base_url");
            let host = base_url
                .split_once("://")
                .map_or(base_url.as_str(), |(_, rest)| rest)
                .split(['/', '?', '#'])
                .next()
                .unwrap_or_default()
                .to_owned();
            json!({
                "id": id,
                "host": host,
                "secret_configured": env::var_os(row.get::<String, _>("api_key_env")).is_some(),
                "models": [],
            })
        });
        if let Some(model) = row.get::<Option<String>, _>("model_id") {
            entry["models"]
                .as_array_mut()
                .expect("models adalah array")
                .push(json!({
                    "id": model,
                    "class": row.get::<Option<String>, _>("class"),
                    "tools": row.get::<String, _>("tools"),
                }));
        }
    }
    Ok(Value::Array(grouped.into_values().collect()))
}

async fn retention(pool: &PgPool) -> Result<Value, sqlx::Error> {
    const TS: &str = "to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')";
    let last_run: Option<String> = sqlx::query_scalar(&format!(
        "SELECT {TS} FROM retention_actions WHERE NOT dry_run ORDER BY id DESC LIMIT 1"
    ))
    .fetch_optional(pool)
    .await?;
    let counts = sqlx::query(
        "SELECT outcome,count(*) AS n FROM retention_actions WHERE NOT dry_run AND created_at > now() - interval '24 hours' GROUP BY 1",
    )
    .fetch_all(pool)
    .await?;
    let mut last_24h = json!({"deleted": 0, "failed": 0});
    for row in counts {
        let outcome: String = row.get("outcome");
        if outcome != "would_delete" {
            last_24h[outcome] = json!(row.get::<i64, _>("n"));
        }
    }
    let recent = sqlx::query(&format!(
        "SELECT {TS} AS at,dry_run,kind,target,outcome,detail FROM retention_actions ORDER BY id DESC LIMIT 20"
    ))
    .fetch_all(pool)
    .await?;
    let recent: Vec<Value> = recent
        .iter()
        .map(|row| {
            // Detail kegagalan dapat memuat path server; tampilkan hanya yang tidak berisi path.
            let detail: Option<String> = row.get("detail");
            json!({
                "at": row.get::<String, _>("at"),
                "dry_run": row.get::<bool, _>("dry_run"),
                "kind": row.get::<String, _>("kind"),
                "target": row.get::<String, _>("target"),
                "outcome": row.get::<String, _>("outcome"),
                "detail": detail.filter(|text| !text.contains('/')),
            })
        })
        .collect();
    Ok(json!({"last_run_at": last_run, "last_24h": last_24h, "recent": recent}))
}
