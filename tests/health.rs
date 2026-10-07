// M5-003: liveness/readiness/metrics terhadap PostgreSQL nyata, termasuk degradasi yang disengaja.
use ai_team::api;
use axum::{Router, middleware};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use sqlx::PgPool;
use tokio::net::TcpListener;
use uuid::Uuid;

const STALE_AFTER: i64 = 60;

async fn server(pool: PgPool) -> (String, Client) {
    let app = Router::new()
        .merge(api::health::router(pool, STALE_AFTER))
        .fallback(api::error::not_found)
        .layer(middleware::from_fn(api::request_id));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, Client::new())
}

async fn json(client: &Client, url: String) -> (StatusCode, Value) {
    let response = client.get(url).send().await.unwrap();
    (response.status(), response.json().await.unwrap())
}

async fn text(client: &Client, url: String) -> (StatusCode, String) {
    let response = client.get(url).send().await.unwrap();
    (response.status(), response.text().await.unwrap())
}

/// Nilai satu seri metrik, mis. `series(&body, "noctis_tasks{status=\"READY\"}")`.
fn series(body: &str, name: &str) -> Option<i64> {
    body.lines()
        .find_map(|line| line.strip_prefix(name)?.trim().parse().ok())
}

async fn seed_run(pool: &PgPool) -> Uuid {
    let (project, run) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query(
        "INSERT INTO projects (id,name,repository_path) VALUES ($1,'health','/tmp/health')",
    )
    .bind(project)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'health','RUNNING',100000)")
        .bind(run)
        .bind(project)
        .execute(pool)
        .await
        .unwrap();
    run
}

async fn seed_task(pool: &PgPool, run: Uuid, id: &str, status: &str) {
    sqlx::query("INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,max_attempts) VALUES ($1,$2,'worker','t','o',$3,'[\"a.txt\"]','[\"ok\"]','[\"true\"]',1000,1000,2)")
        .bind(id)
        .bind(run)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn healthy_database_is_live_ready_and_reports_metrics(pool: PgPool) {
    let (base, client) = server(pool).await;
    let (status, body) = json(&client, format!("{base}/health/live")).await;
    assert_eq!(
        (status, body["status"].as_str()),
        (StatusCode::OK, Some("ok"))
    );
    let (status, body) = json(&client, format!("{base}/health")).await;
    assert_eq!(
        (status, body["database"].as_str()),
        (StatusCode::OK, Some("ok"))
    );
    let (status, body) = json(&client, format!("{base}/health/ready")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ready");
    assert_eq!(body["components"]["database"], "ok");
    assert_eq!(body["components"]["migrations"], "ok");
    // Belum ada model terverifikasi: hanya degraded, bukan tidak siap.
    assert_eq!(body["components"]["providers"], "degraded");
    assert_eq!(body["components"]["workers"], "ok");

    let (status, metrics) = text(&client, format!("{base}/metrics")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(series(&metrics, "noctis_up "), Some(1));
    assert_eq!(
        series(&metrics, "noctis_component_health{component=\"database\"} "),
        Some(2)
    );
    assert_eq!(series(&metrics, "noctis_migrations_pending "), Some(0));
}

#[sqlx::test(migrations = "./migrations")]
async fn liveness_survives_a_dead_database_but_readiness_and_metrics_report_it(pool: PgPool) {
    let (base, client) = server(pool.clone()).await;
    pool.close().await;

    let (status, _) = json(&client, format!("{base}/health/live")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = json(&client, format!("{base}/health/ready")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["components"]["database"], "down");
    // Metrik tetap menjawab dan hanya seri proses/komponen yang ada.
    let (status, metrics) = text(&client, format!("{base}/metrics")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        series(&metrics, "noctis_component_health{component=\"database\"} "),
        Some(0)
    );
    assert!(!metrics.contains("noctis_tasks"));
}

#[sqlx::test(migrations = "./migrations")]
async fn invalid_migration_state_makes_readiness_fail(pool: PgPool) {
    let (base, client) = server(pool.clone()).await;
    let cases: [(&str, &str); 4] = [
        (
            "dirty",
            "UPDATE _sqlx_migrations SET success=false WHERE version=(SELECT max(version) FROM _sqlx_migrations)",
        ),
        (
            "checksum",
            "UPDATE _sqlx_migrations SET checksum=decode('00','hex') WHERE version=(SELECT max(version) FROM _sqlx_migrations)",
        ),
        (
            "unknown",
            "INSERT INTO _sqlx_migrations (version,description,success,checksum,execution_time) VALUES (999999999999,'x',true,decode('00','hex'),0)",
        ),
        (
            "missing",
            "DELETE FROM _sqlx_migrations WHERE version=(SELECT max(version) FROM _sqlx_migrations WHERE version<999999999999)",
        ),
    ];
    // Snapshot keadaan sehat; tiap kasus memutasi lalu mengembalikannya supaya berdiri sendiri.
    sqlx::query("CREATE TABLE migrations_backup AS TABLE _sqlx_migrations")
        .execute(&pool)
        .await
        .unwrap();
    for (name, statement) in cases {
        sqlx::query(statement).execute(&pool).await.unwrap();
        let (status, body) = json(&client, format!("{base}/health/ready")).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{name}");
        assert_eq!(body["components"]["migrations"], "down", "{name}");
        assert_eq!(body["components"]["database"], "ok", "{name}");
        sqlx::query("DELETE FROM _sqlx_migrations")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO _sqlx_migrations SELECT * FROM migrations_backup")
            .execute(&pool)
            .await
            .unwrap();
        let (status, _) = json(&client, format!("{base}/health/ready")).await;
        assert_eq!(status, StatusCode::OK, "{name} harus pulih");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn stale_worker_and_unreachable_provider_degrade_but_do_not_fail_readiness(pool: PgPool) {
    let run = seed_run(&pool).await;
    seed_task(&pool, run, "stale-task", "RUNNING").await;
    // Provider yang tidak terjangkau tidak boleh memengaruhi liveness/readiness.
    sqlx::query("INSERT INTO providers (id,base_url,api_key_env,request_timeout_seconds) VALUES ('down-provider','http://127.0.0.1:1','X',1)")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO models (id,provider_id,remote_name,class,context_window,max_output_tokens) VALUES ('down-model','down-provider','m','coding',1000,1000)")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO agent_runs (id,task_id,role,provider_id,model_id,attempt,status,branch,base_commit,heartbeat_at,retain_until) VALUES ($1,'stale-task','worker','down-provider','down-model',1,'running','b',$2,now()-interval '1 hour',now()+interval '1 hour')")
        .bind(Uuid::new_v4()).bind("0".repeat(40)).execute(&pool).await.unwrap();
    let (base, client) = server(pool).await;

    let (status, body) = json(&client, format!("{base}/health/ready")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["components"]["workers"], "degraded");
    assert_eq!(body["components"]["providers"], "degraded");
    let (_, metrics) = text(&client, format!("{base}/metrics")).await;
    assert_eq!(series(&metrics, "noctis_stale_attempts "), Some(1));
    assert_eq!(
        series(&metrics, "noctis_component_health{component=\"workers\"} "),
        Some(1)
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn metrics_count_state_with_bounded_labels_and_no_identifiers(pool: PgPool) {
    let run = seed_run(&pool).await;
    for (id, status) in [
        ("secret-task-id-1", "READY"),
        ("secret-task-id-2", "READY"),
        ("secret-task-id-3", "DONE"),
    ] {
        seed_task(&pool, run, id, status).await;
    }
    let (base, client) = server(pool).await;
    let (_, metrics) = text(&client, format!("{base}/metrics")).await;

    assert_eq!(series(&metrics, "noctis_tasks{status=\"READY\"} "), Some(2));
    assert_eq!(series(&metrics, "noctis_tasks{status=\"DONE\"} "), Some(1));
    // Status tanpa task tetap muncul (nol), jadi himpunan deret tidak bergantung pada data.
    assert_eq!(
        series(&metrics, "noctis_tasks{status=\"CONFLICT\"} "),
        Some(0)
    );
    assert!(!metrics.contains("secret-task-id"));
    assert!(!metrics.contains(&run.to_string()));

    // Setiap deret hanya memakai nama label dari himpunan tetap.
    let allowed = ["status", "kind", "component", "version"];
    for line in metrics
        .lines()
        .filter(|line| !line.starts_with('#') && line.contains('{'))
    {
        let labels = &line[line.find('{').unwrap() + 1..line.find('}').unwrap()];
        for pair in labels.split(',') {
            assert!(
                allowed.contains(&pair.split('=').next().unwrap()),
                "label tak dikenal: {line}"
            );
        }
    }
    let series_count = metrics
        .lines()
        .filter(|line| !line.starts_with('#'))
        .count();
    // 4 komponen + 15 status task + 5 status attempt + seri skalar: terbatas, tidak tumbuh dengan data.
    assert!(series_count < 50, "{series_count}");
}
