// M4-007B: GET /api/v1/runs/{id}/scheduler (src/store/snapshot.rs) — status scheduler untuk dashboard.
use ai_team::{
    api,
    domain::{
        budget::Purpose,
        task::{AllowedPath, MaxAttempts, NonEmptyString, PositiveLimit, TaskContract, TaskLimits},
    },
    store::{
        budget::{BudgetStore, ReserveRequest},
        event::{AgentAttempt, Usage},
        lease::LeaseStore,
        scheduler::{ClaimRequest, SchedulerStore},
        task::TaskRepository,
    },
};
use axum::{Router, middleware};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::net::TcpListener;
use uuid::Uuid;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn text(value: impl Into<String>) -> NonEmptyString {
    NonEmptyString::parse("value", value).unwrap()
}

fn limit(value: i64) -> PositiveLimit {
    PositiveLimit::new("limit", value).unwrap()
}

async fn server(pool: PgPool, max_slots: usize) -> (String, Client) {
    let app = Router::new()
        .merge(api::projects::router_with_scheduler(pool, None, max_slots))
        .fallback(api::error::not_found)
        .layer(middleware::from_fn(api::request_id));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, Client::new())
}

async fn new_run(pool: &PgPool, budget: i64) -> (Uuid, Uuid) {
    let (project, run) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name,repository_path) VALUES ($1,'project',$2)")
        .bind(project)
        .bind(format!("/repo/{project}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'obj','RUNNING',$3)")
        .bind(run)
        .bind(project)
        .bind(budget)
        .execute(pool)
        .await
        .unwrap();
    (project, run)
}

/// Task manual READY (butuh 150 token; 2 attempt).
async fn ready(
    pool: &PgPool,
    id: &str,
    own: (Uuid, Uuid),
    priority: i32,
    path: &str,
    deps: &[&str],
) {
    TaskRepository::new(pool.clone())
        .create(&TaskContract {
            id: text(id),
            project_id: text(own.0.to_string()),
            project_run_id: text(own.1.to_string()),
            title: text(format!("Title of {id}")),
            role: text("worker"),
            objective: text("ship"),
            depends_on: deps.iter().map(|dep| text(*dep)).collect(),
            allowed_paths: vec![AllowedPath::parse(path).unwrap()],
            context_refs: vec![],
            acceptance_criteria: vec![text("ok")],
            verification_commands: vec![text("cargo test")],
            limits: TaskLimits {
                max_input_tokens: limit(100),
                max_output_tokens: limit(50),
                max_tool_calls: limit(5),
                max_attempts: MaxAttempts::new(2).unwrap(),
                timeout_seconds: limit(60),
            },
        })
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET status='READY',priority=$2 WHERE id=$1")
        .bind(id)
        .bind(priority)
        .execute(pool)
        .await
        .unwrap();
}

async fn snapshot(base: &str, client: &Client, run: Uuid) -> Value {
    let response = client
        .get(format!("{base}/runs/{run}/scheduler"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

fn queue(view: &Value) -> Vec<(String, String, String)> {
    view["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["task_id"].as_str().unwrap().into(),
                item["reason"].as_str().unwrap().into(),
                item["detail"].as_str().unwrap().into(),
            )
        })
        .collect()
}

#[sqlx::test(migrations = "./migrations")]
async fn snapshot_shows_slots_queue_reasons_leases_attempts_and_budget(pool: PgPool) {
    let own = new_run(&pool, 10_000).await;
    ready(&pool, "A", own, 9, "src/a/**", &[]).await;
    ready(&pool, "B", own, 8, "src/b/**", &["A"]).await;
    ready(&pool, "C", own, 7, "src/a/x.rs", &[]).await;
    ready(&pool, "D", own, 6, "src/d/**", &[]).await;
    ready(&pool, "E", own, 5, "src/e/**", &[]).await;

    // A: diklaim, berjalan, memegang lease, sudah memakai 80 token (estimasi) dan menahan 15.
    let owner = Uuid::new_v4();
    let claimed = SchedulerStore::new(pool.clone())
        .claim_next(
            &ClaimRequest {
                owner,
                provider_id: "p",
                model_id: "m",
                retention_seconds: 60,
                only_task: Some("A"),
            },
            |_| Some(COMMIT.to_owned()),
        )
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE tasks SET status='RUNNING' WHERE id='A'")
        .execute(&pool)
        .await
        .unwrap();
    LeaseStore::new(pool.clone())
        .acquire(own.1, "A", owner, &["src/a/**"], 60)
        .await
        .unwrap();
    let tasks = TaskRepository::new(pool.clone());
    tasks
        .record_usage(
            claimed.attempt.id,
            &Usage {
                input_tokens: 60,
                cached_tokens: 5,
                output_tokens: 20,
                tool_calls: 0,
                latency_ms: 1,
                estimated: true,
            },
        )
        .await
        .unwrap();
    BudgetStore::new(pool.clone())
        .reserve(&ReserveRequest {
            attempt_id: claimed.attempt.id,
            request_key: "k",
            input_tokens: 10,
            max_output_tokens: 5,
            purpose: Purpose::Work,
        })
        .await
        .unwrap();
    // E: dua attempt sudah habis.
    for number in 1..=2 {
        tasks
            .create_attempt(&AgentAttempt {
                id: Uuid::new_v4(),
                task_id: text("E"),
                role: text("worker"),
                attempt: number,
                provider_id: text("p"),
                model_id: text("m"),
                status: text("failed"),
            })
            .await
            .unwrap();
    }
    sqlx::query(
        "UPDATE agent_runs SET finished_at=now(),error_code='recovery.stale' WHERE task_id='E'",
    )
    .execute(&pool)
    .await
    .unwrap();

    let (base, client) = server(pool.clone(), 1).await;
    let view = snapshot(&base, &client, own.1).await;

    assert_eq!(
        (view["run_status"].as_str(), view["max_slots"].as_u64()),
        (Some("RUNNING"), Some(1))
    );
    let slot = &view["slots"][0];
    assert_eq!(
        (
            slot["slot"].as_u64(),
            slot["state"].as_str(),
            slot["task_id"].as_str(),
            slot["task_status"].as_str()
        ),
        (Some(1), Some("running"), Some("A"), Some("RUNNING"))
    );
    assert_eq!(
        (
            slot["title"].as_str(),
            slot["attempt"]["number"].as_i64(),
            slot["attempt"]["max"].as_i64()
        ),
        (Some("Title of A"), Some(1), Some(2))
    );
    assert!(
        slot["attempt"]["branch"]
            .as_str()
            .unwrap()
            .starts_with("noctis-A-")
    );
    assert!(slot["heartbeat_age_seconds"].as_i64().unwrap() < 10);
    assert_eq!(slot["leases"][0]["pattern"], "src/a/**");
    assert_eq!(
        slot["tokens"],
        json!({"label":"A","limit":150,"used":80,"held":15,"estimated":true,"reserve":0})
    );
    assert_eq!(view["slots"].as_array().unwrap().len(), 1);

    assert_eq!(
        queue(&view),
        [
            (
                "B".into(),
                "dependency".into(),
                "Waiting for A (running).".into()
            ),
            (
                "C".into(),
                "lease".into(),
                "src/a/** is leased by A.".into()
            ),
            (
                "D".into(),
                "slot".into(),
                "All 1 worker slots are busy.".into()
            ),
            (
                "E".into(),
                "attempts".into(),
                "No attempts left (2 of 2 used).".into()
            ),
        ]
    );
    assert_eq!(
        view["conflicts"],
        json!([{"task_id":"C","pattern":"src/a/**","holder_task_id":"A"}])
    );
    assert_eq!(view["leases"].as_array().unwrap().len(), 1);
    assert_eq!(view["leases"][0]["task_id"], "A");
    assert_eq!(
        view["budget"],
        json!({"label":"Run budget","limit":10000,"used":80,"held":15,"estimated":true,"reserve":1500})
    );
    assert_eq!(
        view["task_budgets"],
        json!([{"label":"A","limit":300,"used":80,"held":15,"estimated":true,"reserve":0}])
    );
    let attempts: Vec<(String, i64, String)> = view["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["task_id"].as_str().unwrap().into(),
                a["number"].as_i64().unwrap(),
                a["status"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(
        attempts,
        [
            ("A".into(), 1, "assigned".into()),
            ("E".into(), 1, "failed".into()),
            ("E".into(), 2, "failed".into())
        ]
    );
    assert_eq!(view["attempts"][1]["error_code"], "recovery.stale");

    // Dengan slot cadangan, kandidat yang layak menunggu tick berikutnya, bukan slot.
    let (wide_base, wide_client) = server(pool.clone(), 4).await;
    let wide = snapshot(&wide_base, &wide_client, own.1).await;
    assert_eq!(
        queue(&wide)[2],
        (
            "D".into(),
            "slot".into(),
            "Ready; it starts on the next scheduler tick.".into()
        )
    );

    // Pause bersifat graceful: slot tetap ada tetapi bertanda pausing, dan semua antrean menunggu resume.
    sqlx::query("UPDATE project_runs SET status='PAUSED' WHERE id=$1")
        .bind(own.1)
        .execute(&pool)
        .await
        .unwrap();
    let paused = snapshot(&base, &client, own.1).await;
    assert_eq!(paused["slots"][0]["state"], "pausing");
    assert!(
        queue(&paused)
            .iter()
            .all(|(_, reason, detail)| reason == "paused" && detail.contains("Resume"))
    );
    sqlx::query("UPDATE project_runs SET status='CANCELLED' WHERE id=$1")
        .bind(own.1)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        snapshot(&base, &client, own.1).await["slots"][0]["state"],
        "cancelling"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn budget_reasons_cover_the_guard_stop_and_unreserved_manual_tasks(pool: PgPool) {
    let (base, client) = server(pool.clone(), 2).await;
    // Guard: batas kerja 850 dari 1.000 sudah terpakai 850 -> task READY menunggu budget.
    let spent = new_run(&pool, 1_000).await;
    ready(&pool, "done-before", spent, 0, "src/h/**", &[]).await;
    let tasks = TaskRepository::new(pool.clone());
    let attempt = AgentAttempt {
        id: Uuid::new_v4(),
        task_id: text("done-before"),
        role: text("worker"),
        attempt: 1,
        provider_id: text("p"),
        model_id: text("m"),
        status: text("completed"),
    };
    tasks.create_attempt(&attempt).await.unwrap();
    tasks
        .record_usage(
            attempt.id,
            &Usage {
                input_tokens: 850,
                cached_tokens: 0,
                output_tokens: 0,
                tool_calls: 0,
                latency_ms: 1,
                estimated: false,
            },
        )
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET status='DONE' WHERE id='done-before'")
        .execute(&pool)
        .await
        .unwrap();
    ready(&pool, "stopped", spent, 5, "src/s/**", &[]).await;
    assert_eq!(
        queue(&snapshot(&base, &client, spent.1).await),
        [(
            "stopped".into(),
            "budget".into(),
            "The run budget has reached its limit for new work.".into()
        )]
    );

    // Task manual yang butuh 150 token pada run bertotal 100 -> tidak muat sama sekali.
    let tiny = new_run(&pool, 100).await;
    ready(&pool, "too-big", tiny, 5, "src/t/**", &[]).await;
    assert_eq!(
        queue(&snapshot(&base, &client, tiny.1).await),
        [(
            "too-big".into(),
            "budget".into(),
            "Needs 150 tokens; 100 remain unreserved.".into()
        )]
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn empty_runs_unknown_runs_and_bad_ids(pool: PgPool) {
    let (base, client) = server(pool.clone(), 3).await;
    let own = new_run(&pool, 5_000).await;
    let view = snapshot(&base, &client, own.1).await;
    assert_eq!(
        (
            view["slots"].as_array().unwrap().len(),
            view["queue"].as_array().unwrap().len(),
            view["max_slots"].as_u64()
        ),
        (0, 0, Some(3))
    );
    assert_eq!(
        view["budget"],
        json!({"label":"Run budget","limit":5000,"used":0,"held":0,"estimated":false,"reserve":750})
    );

    assert_eq!(
        client
            .get(format!("{base}/runs/{}/scheduler", Uuid::new_v4()))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        client
            .get(format!("{base}/runs/not-a-uuid/scheduler"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Hanya membaca: tidak ada event atau perubahan state yang dibuat.
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}
