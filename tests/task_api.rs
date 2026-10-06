use std::{path::PathBuf, time::Duration};

use ai_team::{api, api::tasks::StartState, store::artifact::ArtifactStore};
use axum::{Router, http::HeaderValue, middleware};
use reqwest::{Client, Response, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle};
use uuid::Uuid;

struct Server {
    base: String,
    client: Client,
    handle: JoinHandle<()>,
    artifact_root: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.handle.abort();
        let _ = std::fs::remove_dir_all(&self.artifact_root);
    }
}

async fn server(pool: PgPool) -> Server {
    sqlx::query("INSERT INTO providers (id,base_url,api_key_env,request_timeout_seconds) VALUES ('api-provider','http://127.0.0.1:1','TEST_API_KEY',1)")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO models (id,provider_id,remote_name,class,context_window,max_output_tokens,claimed_capabilities,verified_capabilities) VALUES ('api-model','api-provider','mock','coding',1000,1000,'{\"chat\":true,\"streaming\":false,\"tools\":true,\"parallel_tools\":false}','{\"chat\":\"unknown\",\"streaming\":\"unknown\",\"tools\":\"unknown\",\"parallel_tools\":\"unknown\"}')")
        .execute(&pool).await.unwrap();
    let artifact_root = std::env::temp_dir().join(format!("noctis-task-api-{}", Uuid::new_v4()));
    let artifacts = ArtifactStore::new(&artifact_root, 1_048_576).unwrap();
    let (wake, _receiver) = mpsc::channel(1);
    let app = Router::new()
        .merge(api::tasks::router_with_start(
            pool,
            artifacts,
            Some(StartState {
                model_id: "api-model".to_owned(),
                retention_seconds: 60,
                wake,
            }),
        ))
        .fallback(api::error::not_found)
        .layer(api::cors_layer(
            "http://127.0.0.1:5173".parse::<HeaderValue>().unwrap(),
        ))
        .layer(middleware::from_fn(api::request_id));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server {
        base: format!("http://{address}/api/v1"),
        client: Client::new(),
        handle,
        artifact_root,
    }
}

async fn ownership(pool: &PgPool) -> (Uuid, Uuid) {
    let project_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    let repository = std::env::temp_dir().join(format!("noctis-task-repository-{project_id}"));
    std::fs::create_dir_all(&repository).unwrap();
    for arguments in [
        vec!["init", "--initial-branch=main"],
        vec!["config", "user.name", "Noctis Test"],
        vec!["config", "user.email", "noctis@example.invalid"],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repository)
                .args(arguments)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(repository.join("tracked.txt"), "base\n").unwrap();
    for arguments in [vec!["add", "tracked.txt"], vec!["commit", "-m", "initial"]] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repository)
                .args(arguments)
                .status()
                .unwrap()
                .success()
        );
    }
    sqlx::query("INSERT INTO projects (id,name,repository_path) VALUES ($1,'api',$2)")
        .bind(project_id)
        .bind(repository.to_string_lossy().as_ref())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'api','running',100)")
        .bind(run_id).bind(project_id).execute(pool).await.unwrap();
    (project_id, run_id)
}

fn task(id: &str, project_id: Uuid, run_id: Uuid) -> Value {
    json!({
        "id":id,"project_id":project_id,"project_run_id":run_id,
        "title":"Task API","role":"worker","objective":"Verify API",
        "depends_on":[],"allowed_paths":["src/**"],"context_refs":[],
        "acceptance_criteria":["works"],"verification_commands":["cargo test"],
        "limits":{"max_input_tokens":1000,"max_output_tokens":1000,"max_tool_calls":10,"max_attempts":2,"timeout_seconds":60}
    })
}

async fn mutation(
    client: &Client,
    method: reqwest::Method,
    url: &str,
    key: &str,
    body: Option<&Value>,
) -> Response {
    let request = client.request(method, url).header("Idempotency-Key", key);
    match body {
        Some(body) => request.json(body),
        None => request,
    }
    .send()
    .await
    .unwrap()
}

async fn delete(client: &Client, url: &str, key: &str, expected_version: i64) -> Response {
    mutation(
        client,
        reqwest::Method::DELETE,
        url,
        key,
        Some(&json!({"expected_version":expected_version})),
    )
    .await
}

fn assert_request_id(response: &Response) {
    assert!(
        Uuid::parse_str(
            response
                .headers()
                .get("x-request-id")
                .unwrap()
                .to_str()
                .unwrap()
        )
        .is_ok()
    );
}

async fn next_sse_event(response: &mut Response) -> String {
    let mut event = String::new();
    loop {
        let chunk = tokio::time::timeout(Duration::from_secs(3), response.chunk())
            .await
            .expect("SSE event timed out")
            .unwrap()
            .expect("SSE stream closed");
        event.push_str(std::str::from_utf8(&chunk).unwrap());
        if let Some(end) = event.find("\n\n") {
            return event[..end + 2].to_owned();
        }
    }
}

fn sse_id(event: &str) -> i64 {
    event
        .lines()
        .find_map(|line| line.strip_prefix("id: "))
        .unwrap()
        .parse()
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn crud_pagination_idempotency_validation_and_request_id(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let server = server(pool).await;
    let tasks = format!("{}/tasks", server.base);
    let body = task("api-a", project_id, run_id);

    let created = mutation(
        &server.client,
        reqwest::Method::POST,
        &tasks,
        "create-a",
        Some(&body),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_request_id(&created);
    assert_eq!(
        created.headers()["access-control-expose-headers"],
        "x-request-id"
    );
    let created_body: Value = created.json().await.unwrap();
    assert_eq!(
        created_body["contract"]["project_id"],
        project_id.to_string()
    );
    assert_eq!(
        created_body["contract"]["project_run_id"],
        run_id.to_string()
    );

    let replay = mutation(
        &server.client,
        reqwest::Method::POST,
        &tasks,
        "create-a",
        Some(&body),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::CREATED);
    assert_eq!(replay.json::<Value>().await.unwrap(), created_body);
    let conflict = mutation(
        &server.client,
        reqwest::Method::POST,
        &tasks,
        "create-a",
        Some(&task("api-b", project_id, run_id)),
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let invalid = mutation(
        &server.client,
        reqwest::Method::POST,
        &tasks,
        "invalid",
        Some(&json!({"id":"bad"})),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let invalid_body: Value = invalid.json().await.unwrap();
    assert!(invalid_body["error"]["request_id"].as_str().is_some());

    let created_b = mutation(
        &server.client,
        reqwest::Method::POST,
        &tasks,
        "create-b",
        Some(&task("api-b", project_id, run_id)),
    )
    .await;
    assert_eq!(
        created_b.status(),
        StatusCode::CREATED,
        "{}",
        created_b.text().await.unwrap()
    );
    let page: Value = server
        .client
        .get(format!("{tasks}?limit=1"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let cursor = page["next_cursor"].as_str().unwrap();
    let next: Value = server
        .client
        .get(format!("{tasks}?limit=1&cursor={cursor}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(next["items"].as_array().unwrap().len(), 1);
    // Filter per run: run milik task ini mengembalikan semuanya; run lain mengembalikan kosong.
    for (run, expected) in [(run_id, 2), (uuid::Uuid::new_v4(), 0)] {
        let filtered: Value = server
            .client
            .get(format!("{tasks}?project_run_id={run}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(filtered["items"].as_array().unwrap().len(), expected);
    }

    let mut update = body.clone();
    update["expected_version"] = json!(0);
    update["title"] = json!("Updated");
    let updated = mutation(
        &server.client,
        reqwest::Method::PUT,
        &format!("{tasks}/api-a"),
        "update",
        Some(&update),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let stale = mutation(
        &server.client,
        reqwest::Method::PUT,
        &format!("{tasks}/api-a"),
        "stale",
        Some(&update),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let missing = server
        .client
        .get(format!("{tasks}/missing"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let missing_text = missing.text().await.unwrap();
    assert!(!missing_text.contains("postgres"));
    assert!(!missing_text.contains("/repository/"));

    let stale_delete = delete(&server.client, &format!("{tasks}/api-a"), "delete-stale", 0).await;
    assert_eq!(stale_delete.status(), StatusCode::CONFLICT);

    let missing_version = mutation(
        &server.client,
        reqwest::Method::DELETE,
        &format!("{tasks}/api-b"),
        "delete-no-version",
        Some(&json!({})),
    )
    .await;
    assert_eq!(missing_version.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let missing_delete = delete(
        &server.client,
        &format!("{tasks}/missing"),
        "delete-missing",
        0,
    )
    .await;
    assert_eq!(missing_delete.status(), StatusCode::NOT_FOUND);

    let deleted = delete(&server.client, &format!("{tasks}/api-b"), "delete", 0).await;
    assert_eq!(deleted.status(), StatusCode::OK);
    let replayed = delete(&server.client, &format!("{tasks}/api-b"), "delete", 0).await;
    assert_eq!(replayed.status(), StatusCode::OK);

    let preflight = server
        .client
        .request(reqwest::Method::OPTIONS, &tasks)
        .header("Origin", "http://127.0.0.1:5173")
        .header("Access-Control-Request-Method", "POST")
        .header(
            "Access-Control-Request-Headers",
            "content-type,idempotency-key,last-event-id",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(preflight.status(), StatusCode::OK);
    let allow_headers = preflight.headers()["access-control-allow-headers"]
        .to_str()
        .unwrap();
    assert!(allow_headers.contains("idempotency-key"));
    assert!(allow_headers.contains("last-event-id"));
}

#[sqlx::test(migrations = "./migrations")]
async fn canonical_uuid_and_transition_contract(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let server = server(pool.clone()).await;
    let tasks = format!("{}/tasks", server.base);
    for (field, uuid) in [("project_id", project_id), ("project_run_id", run_id)] {
        for (index, value) in [
            uuid.to_string().to_uppercase(),
            uuid.simple().to_string(),
            uuid.braced().to_string(),
            uuid.urn().to_string(),
        ]
        .into_iter()
        .enumerate()
        {
            let mut body = task(&format!("invalid-{field}-{index}"), project_id, run_id);
            body[field] = json!(value);
            assert_eq!(
                mutation(
                    &server.client,
                    reqwest::Method::POST,
                    &tasks,
                    &format!("uuid-{field}-{index}"),
                    Some(&body)
                )
                .await
                .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
        }
    }
    let body = task("transition", project_id, run_id);
    assert_eq!(
        mutation(
            &server.client,
            reqwest::Method::POST,
            &tasks,
            "transition-create",
            Some(&body)
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let invalid = mutation(
        &server.client,
        reqwest::Method::POST,
        &format!("{tasks}/transition/start"),
        "bad-start",
        Some(&json!({"expected_version":0})),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::CONFLICT);

    sqlx::query("UPDATE tasks SET status='READY' WHERE id='transition'")
        .execute(&pool)
        .await
        .unwrap();
    let started = mutation(
        &server.client,
        reqwest::Method::POST,
        &format!("{tasks}/transition/start"),
        "start",
        Some(&json!({"expected_version":0})),
    )
    .await;
    assert_eq!(started.status(), StatusCode::OK);
    assert_eq!(started.json::<Value>().await.unwrap()["status"], "ASSIGNED");
    let cancelled = mutation(
        &server.client,
        reqwest::Method::POST,
        &format!("{tasks}/transition/cancel"),
        "cancel",
        Some(&json!({"expected_version":1})),
    )
    .await;
    assert_eq!(cancelled.status(), StatusCode::OK);
    let retry_invalid = mutation(
        &server.client,
        reqwest::Method::POST,
        &format!("{tasks}/transition/retry"),
        "retry-invalid",
        Some(&json!({"expected_version":2})),
    )
    .await;
    assert_eq!(retry_invalid.status(), StatusCode::CONFLICT);
    sqlx::query("UPDATE tasks SET status='FAILED' WHERE id='transition'")
        .execute(&pool)
        .await
        .unwrap();
    let retried = mutation(
        &server.client,
        reqwest::Method::POST,
        &format!("{tasks}/transition/retry"),
        "retry",
        Some(&json!({"expected_version":2})),
    )
    .await;
    assert_eq!(retried.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "./migrations")]
async fn events_sse_reconnect_and_artifacts_redact_internal_paths(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let server = server(pool.clone()).await;
    let tasks = format!("{}/tasks", server.base);
    mutation(
        &server.client,
        reqwest::Method::POST,
        &tasks,
        "event-create",
        Some(&task("evidence", project_id, run_id)),
    )
    .await;
    sqlx::query("UPDATE tasks SET status='READY' WHERE id='evidence'")
        .execute(&pool)
        .await
        .unwrap();
    let client = server.client.clone();
    let stream_url = format!("{tasks}/evidence/events/stream");
    let initial_request = tokio::spawn(async move { client.get(stream_url).send().await.unwrap() });
    tokio::time::sleep(Duration::from_millis(300)).await;
    mutation(
        &server.client,
        reqwest::Method::POST,
        &format!("{tasks}/evidence/start"),
        "event-start",
        Some(&json!({"expected_version":0})),
    )
    .await;
    let mut initial = tokio::time::timeout(Duration::from_secs(3), initial_request)
        .await
        .expect("SSE response timed out")
        .unwrap();
    assert_eq!(initial.headers()["content-type"], "text/event-stream");
    let first = next_sse_event(&mut initial).await;
    assert!(first.contains("event: task_event"));
    let first_id = sse_id(&first);
    drop(initial);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let client = server.client.clone();
    let stream_url = format!("{tasks}/evidence/events/stream");
    let reconnect_request = tokio::spawn(async move {
        client
            .get(stream_url)
            .header("Last-Event-ID", first_id)
            .send()
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    mutation(
        &server.client,
        reqwest::Method::POST,
        &format!("{tasks}/evidence/cancel"),
        "event-cancel",
        Some(&json!({"expected_version":1})),
    )
    .await;
    let mut reconnect = tokio::time::timeout(Duration::from_secs(3), reconnect_request)
        .await
        .expect("SSE reconnect timed out")
        .unwrap();
    let second = next_sse_event(&mut reconnect).await;
    let second_id = sse_id(&second);
    assert!(second_id > first_id);
    assert!(!second.contains(&format!("id: {first_id}\n")));
    assert!(
        tokio::time::timeout(Duration::from_millis(600), reconnect.chunk())
            .await
            .is_err()
    );

    let events: Value = server
        .client
        .get(format!("{tasks}/evidence/events"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = events["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], first_id);
    assert_eq!(items[1]["id"], second_id);

    let artifact_id = Uuid::new_v4();
    let bytes = b"diff --git a/safe b/safe\n+safe\n";
    let store = ArtifactStore::new(&server.artifact_root, 1_048_576).unwrap();
    let metadata = store
        .write(
            &artifact_id.to_string(),
            "change.diff",
            "text/x-diff",
            bytes,
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    sqlx::query("INSERT INTO artifacts (id,task_id,kind,path,size_bytes,sha256) VALUES ($1,'evidence','diff',$2,$3,$4)")
        .bind(artifact_id).bind("/host/private/secret.diff").bind(metadata.size as i64).bind(format!("{:x}",Sha256::digest(bytes))).execute(&pool).await.unwrap();
    let listed = server
        .client
        .get(format!("{tasks}/evidence/artifacts"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!listed.contains("path"));
    assert!(!listed.contains("/host/private"));
    let downloaded = server
        .client
        .get(format!("{tasks}/evidence/artifacts/{artifact_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(downloaded.bytes().await.unwrap().as_ref(), bytes);
    let diff = server
        .client
        .get(format!("{tasks}/evidence/diff"))
        .send()
        .await
        .unwrap();
    assert_eq!(diff.bytes().await.unwrap().as_ref(), bytes);
    assert_eq!(
        server
            .client
            .get(format!("{tasks}/evidence/artifacts/../../etc/passwd"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}
