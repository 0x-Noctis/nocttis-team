// M5-008: endpoint ringkasan operasional dan konfigurasi non-secret (PostgreSQL nyata).
use ai_team::api;
use axum::{Router, middleware};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::net::TcpListener;
use uuid::Uuid;

async fn server(pool: PgPool, config: Value) -> (String, Client) {
    let app = Router::new()
        .merge(api::operations::router(pool, 60, config))
        .fallback(api::error::not_found)
        .layer(middleware::from_fn(api::request_id));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!(
        "http://{}/api/v1/operations",
        listener.local_addr().unwrap()
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, Client::new())
}

async fn get(client: &Client, url: &str) -> (StatusCode, Value, String) {
    let response = client.get(url).send().await.unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap(), text)
}

async fn seed(pool: &PgPool) -> Uuid {
    let (project, run, attempt) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name,repository_path) VALUES ($1,'ops','/tmp/ops')")
        .bind(project)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'ops','RUNNING',100000)")
        .bind(run).bind(project).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO providers (id,base_url,api_key_env,request_timeout_seconds) VALUES ('with-secret','https://api.example.test:8443/v1?k=1','NOCTIS_OPS_TEST_KEY',5)")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO providers (id,base_url,api_key_env,request_timeout_seconds) VALUES ('no-secret','http://127.0.0.1:1','NOCTIS_OPS_MISSING_KEY',5)")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO models (id,provider_id,remote_name,class,context_window,max_output_tokens,verified_capabilities) VALUES ('m1','with-secret','m','coding',1000,1000,'{\"chat\":\"unknown\",\"streaming\":\"unknown\",\"tools\":\"supported\",\"parallel_tools\":\"unknown\"}')")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO models (id,provider_id,remote_name,class,context_window,max_output_tokens) VALUES ('m2','with-secret','m2','coding',1000,1000)")
        .execute(pool).await.unwrap();
    for (id, status) in [
        ("t-ready", "READY"),
        ("t-done", "DONE"),
        ("t-human", "NEEDS_HUMAN"),
    ] {
        sqlx::query("INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,max_attempts) VALUES ($1,$2,'worker','t','o',$3,'[\"a\"]','[\"ok\"]','[\"true\"]',1000,1000,2)")
            .bind(id).bind(run).bind(status).execute(pool).await.unwrap();
    }
    sqlx::query("INSERT INTO agent_runs (id,task_id,role,provider_id,model_id,attempt,status,branch,base_commit,heartbeat_at,finished_at,retain_until) VALUES ($1,'t-done','worker','with-secret','m1',1,'completed','b',$2,now(),now(),now())")
        .bind(attempt).bind("0".repeat(40)).execute(pool).await.unwrap();
    // 1.000 token tercatat dari provider, 500 token estimasi, satu baris dengan biaya.
    sqlx::query("INSERT INTO model_usage (agent_run_id,input_tokens,output_tokens,latency_ms,estimated,cost_micros) VALUES ($1,600,400,1,false,2500)")
        .bind(attempt).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO model_usage (agent_run_id,input_tokens,output_tokens,latency_ms,estimated) VALUES ($1,300,200,1,true)")
        .bind(attempt).execute(pool).await.unwrap();
    run
}

#[sqlx::test(migrations = "./migrations")]
async fn summary_reports_health_usage_providers_and_retention_without_secrets(pool: PgPool) {
    let secret = "ops-test-secret-value-424242";
    unsafe { std::env::set_var("NOCTIS_OPS_TEST_KEY", secret) };
    unsafe { std::env::remove_var("NOCTIS_OPS_MISSING_KEY") };
    seed(&pool).await;
    sqlx::query("INSERT INTO retention_actions (dry_run,kind,target,outcome,detail) VALUES (false,'orphan_artifact','a-1','deleted',NULL),(false,'integration_worktree','run-1','failed','git: /home/me/secret/path failed'),(true,'orphan_artifact','a-2','would_delete',NULL)")
        .execute(&pool).await.unwrap();
    let (base, client) = server(pool, json!({})).await;

    let (status, body, text) = get(&client, &base).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ready"], true);
    assert_eq!(body["components"]["database"], "ok");
    // Setiap status task/attempt hadir (nol bila kosong).
    assert_eq!(body["tasks"]["READY"], 1);
    assert_eq!(body["tasks"]["NEEDS_HUMAN"], 1);
    assert_eq!(body["tasks"]["CONFLICT"], 0);
    assert_eq!(body["attempts"]["completed"], 1);
    assert_eq!(body["attempts"]["running"], 0);
    // Pemakaian: total dan bagian estimasi terpisah; biaya hanya dari baris yang punya harga.
    assert_eq!(body["usage"]["input_tokens"], 900);
    assert_eq!(body["usage"]["output_tokens"], 600);
    assert_eq!(body["usage"]["estimated_tokens"], 500);
    assert_eq!(
        body["usage"]["cost"],
        json!({"tracked": true, "micros": 2500, "priced_rows": 1, "total_rows": 2})
    );
    // Provider: host saja (tanpa query/skema), kesiapan secret hanya boolean, kemampuan tools per model.
    let providers = body["providers"].as_array().unwrap();
    let with = providers.iter().find(|p| p["id"] == "with-secret").unwrap();
    assert_eq!(with["host"], "api.example.test:8443");
    assert_eq!(with["secret_configured"], true);
    assert_eq!(with["models"].as_array().unwrap().len(), 2);
    assert_eq!(with["models"][0]["tools"], "supported");
    assert_eq!(with["models"][1]["tools"], "unknown");
    let without = providers.iter().find(|p| p["id"] == "no-secret").unwrap();
    assert_eq!(without["secret_configured"], false);
    assert_eq!(without["models"], json!([]));
    // Retensi: ringkasan 24 jam tidak menghitung dry-run; detail berisi path disembunyikan.
    assert_eq!(
        body["retention"]["last_24h"],
        json!({"deleted": 1, "failed": 1})
    );
    assert!(body["retention"]["last_run_at"].is_string());
    let recent = body["retention"]["recent"].as_array().unwrap();
    assert_eq!(recent.len(), 3);
    assert!(
        recent.iter().all(|row| row["detail"].is_null()),
        "detail berpath tidak boleh keluar"
    );
    assert_eq!(recent[0]["dry_run"], true);
    // Tidak ada secret, query string provider, atau path server di respons.
    for forbidden in [secret, "k=1", "/home/me", "NOCTIS_OPS_TEST_KEY"] {
        assert!(!text.contains(forbidden), "bocor: {forbidden}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn config_endpoint_returns_exactly_what_main_provides(pool: PgPool) {
    let config =
        json!({"scheduler": {"max_parallel_agents": 4}, "runner": {"network_enabled": false}});
    let (base, client) = server(pool, config.clone()).await;
    let (status, body, _) = get(&client, &format!("{base}/config")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, config);
}

#[sqlx::test(migrations = "./migrations")]
async fn unreachable_database_is_reported_as_down_with_empty_sections(pool: PgPool) {
    let (base, client) = server(pool.clone(), json!({})).await;
    pool.close().await;
    let (status, body, _) = get(&client, &base).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ready"], false);
    assert_eq!(body["components"]["database"], "down");
    for section in ["tasks", "attempts", "usage", "providers", "retention"] {
        assert!(body[section].is_null(), "{section}");
    }
}
