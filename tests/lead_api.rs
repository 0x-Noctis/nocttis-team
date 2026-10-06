// M3-006B: POST /runs/{id}/lead-plan memanggil Lead Agent dan menyimpan plan PROPOSED.
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
    thread,
};

use ai_team::{api, context::discovery::discover};
use axum::{Router, middleware};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::net::TcpListener as TokioListener;
use uuid::Uuid;

const KEY_ENV: &str = "LEAD_API_TEST_KEY";

/// Server OpenAI-compatible minimal: tiap koneksi dijawab dengan body berikutnya dari antrean
/// (status 200 kecuali body diawali "!500"); `hits` menghitung request yang masuk.
fn mock_model(replies: Vec<String>) -> (String, Arc<Mutex<usize>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let hits = Arc::new(Mutex::new(0));
    let counter = hits.clone();
    thread::spawn(move || {
        for reply in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buffer = Vec::new();
            let mut chunk = [0_u8; 4096];
            // Baca header lalu body sesuai Content-Length supaya client tidak menerima reset.
            loop {
                let read = stream.read(&mut chunk).unwrap_or(0);
                buffer.extend_from_slice(&chunk[..read]);
                let text = String::from_utf8_lossy(&buffer).to_lowercase();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if buffer.len() >= end + 4 + length || read == 0 {
                        break;
                    }
                } else if read == 0 {
                    break;
                }
            }
            *counter.lock().unwrap() += 1;
            let (status, body) = match reply.strip_prefix("!500") {
                Some(_) => ("500 Internal Server Error", "{}".to_owned()),
                None => ("200 OK", chat_reply(&reply)),
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (url, hits)
}

fn chat_reply(content: &str) -> String {
    json!({
        "choices":[{"message":{"content":content},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}
    })
    .to_string()
}

/// Repository Git sementara berisi Cargo.toml supaya discovery menemukan test command.
fn temp_repo() -> (PathBuf, String) {
    let root = std::env::temp_dir().join(format!("lead-api-{}", Uuid::new_v4()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname=\"fixture\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "").unwrap();
    assert!(
        Command::new("git")
            .arg("init")
            .arg(&root)
            .output()
            .unwrap()
            .status
            .success()
    );
    let command = discover(&root).unwrap().test_commands.remove(0);
    (std::fs::canonicalize(&root).unwrap(), command)
}

/// Plan yang "ditulis model": ID sengaja pendek dan sama untuk setiap run ("t1", "t2").
fn plan_json(project: Uuid, run: Uuid, command: &str) -> String {
    let task = |id: &str, path: &str, deps: Value| {
        json!({
            "id":id,"project_id":project,"project_run_id":run,"title":format!("Task {id}"),
            "role":"worker","objective":"Do it","depends_on":deps,"allowed_paths":[path],
            "context_refs":[],"acceptance_criteria":["Works"],"verification_commands":[command],
            "limits":{"max_input_tokens":20,"max_output_tokens":10,"max_tool_calls":1,"max_attempts":1,"timeout_seconds":10}
        })
    };
    json!({
        "id":"plan-1","project_run_id":run,"version":1,"risk_flags":["touches shared module"],
        "tasks":[task("t1","src/a.rs",json!([])), task("t2","src/b.rs",json!(["t1"]))]
    })
    .to_string()
}

async fn server(pool: PgPool, default_model: Option<&str>) -> (String, Client) {
    let app = Router::new()
        .merge(api::projects::router_with_default_model(
            pool,
            default_model.map(str::to_owned),
        ))
        .fallback(api::error::not_found)
        .layer(middleware::from_fn(api::request_id));
    let listener = TokioListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, Client::new())
}

async fn send(
    client: &Client,
    method: Method,
    url: &str,
    key: &str,
    value: Value,
) -> (StatusCode, Value) {
    let response = client
        .request(method, url)
        .header("Idempotency-Key", key)
        .json(&value)
        .send()
        .await
        .unwrap();
    (response.status(), response.json().await.unwrap())
}

async fn register_model(pool: &PgPool, base_url: &str, key_env: &str) {
    sqlx::query("INSERT INTO providers (id,base_url,api_key_env,request_timeout_seconds) VALUES ('lead-provider',$1,$2,5)")
        .bind(base_url).bind(key_env).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO models (id,provider_id,remote_name,class,context_window,max_output_tokens) VALUES ('lead-model','lead-provider','mock','reasoning',8000,1000)")
        .execute(pool).await.unwrap();
}

/// Buat project + satu run baru di server; mengembalikan (project, run).
async fn project_with_run(
    base: &str,
    client: &Client,
    repo: &PathBuf,
    project: Option<Uuid>,
) -> (Uuid, Uuid) {
    let project = match project {
        Some(existing) => existing,
        None => {
            let id = Uuid::new_v4();
            let (status, body) = send(
                client,
                Method::POST,
                &format!("{base}/projects"),
                &Uuid::new_v4().to_string(),
                json!({"id":id,"name":"Lead project","repository_path":repo}),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
            id
        }
    };
    let run = Uuid::new_v4();
    let (status, body) = send(client, Method::POST, &format!("{base}/projects/{project}/runs"), &Uuid::new_v4().to_string(),
        json!({"id":run,"project_id":project,"objective":"Ship","acceptance_criteria":["Tests pass"],"token_budget":1000})).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (project, run)
}

#[sqlx::test(migrations = "./migrations")]
async fn lead_plan_is_namespaced_proposed_and_approvable_across_runs(pool: PgPool) {
    unsafe { std::env::set_var(KEY_ENV, "test-only") };
    let (repo, command) = temp_repo();
    let (base, client) = server(pool.clone(), Some("lead-model")).await;
    let (project, run_a) = project_with_run(&base, &client, &repo, None).await;
    let (_, run_b) = project_with_run(&base, &client, &repo, Some(project)).await;
    // Dua run memakai ID task yang sama ("t1"/"t2") dari model: harus tetap tidak bertabrakan.
    let (url, hits) = mock_model(vec![
        plan_json(project, run_a, &command),
        plan_json(project, run_b, &command),
        plan_json(project, run_a, &command),
    ]);
    register_model(&pool, &url, KEY_ENV).await;

    let lead = |run: Uuid| format!("{base}/runs/{run}/lead-plan");
    let (status, planned) = send(&client, Method::POST, &lead(run_a), "lead-a", json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{planned}");
    let plan = &planned["plan"];
    let prefix = &run_a.to_string()[..8];
    assert_eq!(plan["status"], "PROPOSED");
    assert_eq!(plan["version"], 1);
    assert_eq!(plan["id"], json!(format!("plan-{run_a}-v1")));
    assert_eq!(plan["tasks"][0]["id"], json!(format!("{prefix}-t1")));
    assert_eq!(
        plan["tasks"][1]["depends_on"],
        json!([format!("{prefix}-t1")])
    );
    assert_eq!(plan["risk_flags"], json!(["touches shared module"]));

    // Replay idempotent tidak memanggil model lagi; plan kedua selagi PROPOSED ditolak tanpa memanggil model.
    assert_eq!(
        send(&client, Method::POST, &lead(run_a), "lead-a", json!({})).await,
        (StatusCode::CREATED, planned.clone())
    );
    assert_eq!(
        send(
            &client,
            Method::POST,
            &lead(run_a),
            "lead-a-again",
            json!({})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(*hits.lock().unwrap(), 1);

    let (status, _) = send(
        &client,
        Method::POST,
        &lead(run_b),
        "lead-b",
        json!({"model_id":"lead-model"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    for (run, key) in [(run_a, "approve-a"), (run_b, "approve-b")] {
        let plan_id = format!("plan-{run}-v1");
        let (status, body) = send(
            &client,
            Method::POST,
            &format!("{base}/runs/{run}/approve-plan"),
            key,
            json!({"plan_id":plan_id,"actor_id":"human","decision":"APPROVED"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tasks")
            .fetch_one(&pool)
            .await
            .unwrap(),
        4
    );

    // Run yang sudah RUNNING tidak boleh direncanakan ulang oleh Lead.
    assert_eq!(
        send(
            &client,
            Method::POST,
            &lead(run_a),
            "lead-a-running",
            json!({})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(*hits.lock().unwrap(), 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn rejected_plan_can_be_replanned_as_next_version(pool: PgPool) {
    unsafe { std::env::set_var(KEY_ENV, "test-only") };
    let (repo, command) = temp_repo();
    let (base, client) = server(pool.clone(), Some("lead-model")).await;
    let (project, run) = project_with_run(&base, &client, &repo, None).await;
    let (url, _) = mock_model(vec![
        plan_json(project, run, &command),
        plan_json(project, run, &command),
    ]);
    register_model(&pool, &url, KEY_ENV).await;
    let lead = format!("{base}/runs/{run}/lead-plan");

    assert_eq!(
        send(&client, Method::POST, &lead, "first", json!({}))
            .await
            .0,
        StatusCode::CREATED
    );
    let (status, _) = send(&client, Method::POST, &format!("{base}/runs/{run}/reject-plan"), "reject",
        json!({"plan_id":format!("plan-{run}-v1"),"actor_id":"human","decision":"REJECTED","reason":"too broad"})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, second) = send(&client, Method::POST, &lead, "second", json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    assert_eq!(second["plan"]["version"], 2);
    assert_eq!(second["plan"]["id"], json!(format!("plan-{run}-v2")));
}

#[sqlx::test(migrations = "./migrations")]
async fn lead_plan_failures_map_to_safe_errors(pool: PgPool) {
    unsafe { std::env::set_var(KEY_ENV, "test-only") };
    let (repo, _) = temp_repo();
    let (base, client) = server(pool.clone(), Some("lead-model")).await;
    let (project, run) = project_with_run(&base, &client, &repo, None).await;
    let url_for = |run: Uuid| format!("{base}/runs/{run}/lead-plan");

    // Model belum terdaftar -> 422; tanpa default dan tanpa model_id -> 422.
    assert_eq!(
        send(&client, Method::POST, &url_for(run), "no-model", json!({}))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (bare, _) = server(pool.clone(), None).await;
    assert_eq!(
        send(
            &client,
            Method::POST,
            &format!("{bare}/runs/{run}/lead-plan"),
            "no-default",
            json!({})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // Secret provider tidak ada di environment -> 409 tanpa membocorkan nama/nilai.
    let (url, hits) = mock_model(vec![
        "not json".into(),
        "!500".into(),
        plan_json(project, run, "rm -rf /"),
    ]);
    register_model(&pool, &url, "LEAD_API_MISSING_KEY").await;
    let (status, body) = send(&client, Method::POST, &url_for(run), "no-secret", json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(!body.to_string().contains("LEAD_API_MISSING_KEY"));
    assert_eq!(*hits.lock().unwrap(), 0);

    sqlx::query("UPDATE providers SET api_key_env=$1")
        .bind(KEY_ENV)
        .execute(&pool)
        .await
        .unwrap();
    // Output bukan JSON plan -> 422; provider 500 -> 502; command di luar discovery -> 422.
    let (status, body) = send(&client, Method::POST, &url_for(run), "bad-json", json!({})).await;
    assert_eq!(
        (status, body["error"]["details"]["lead"].as_str()),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Some("lead response is invalid")
        )
    );
    let (status, body) = send(
        &client,
        Method::POST,
        &url_for(run),
        "provider-down",
        json!({}),
    )
    .await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (StatusCode::BAD_GATEWAY, Some("BAD_GATEWAY"))
    );
    let (status, body) = send(
        &client,
        Method::POST,
        &url_for(run),
        "bad-command",
        json!({}),
    )
    .await;
    assert_eq!(
        (status, body["error"]["details"]["lead"].as_str()),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Some("lead plan contains unapproved verification command")
        )
    );
    // Tidak satu pun plan tersimpan dari kegagalan di atas; run tetap PLANNING.
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM plans")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM project_runs WHERE id=$1")
            .bind(run)
            .fetch_one(&pool)
            .await
            .unwrap(),
        "PLANNING"
    );
    // Run tak dikenal -> 404.
    assert_eq!(
        send(
            &client,
            Method::POST,
            &url_for(Uuid::new_v4()),
            "ghost",
            json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}
