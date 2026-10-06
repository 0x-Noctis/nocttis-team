// M4-009B: antrean keputusan manusia (GET /approvals) dan aksi task beridentitas.
use ai_team::{
    api,
    domain::task::{
        AllowedPath, MaxAttempts, NonEmptyString, PositiveLimit, TaskContract, TaskLimits,
    },
    store::{artifact::ArtifactStore, event::AgentAttempt, task::TaskRepository},
};
use axum::{Router, middleware};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::net::TcpListener;
use uuid::Uuid;

fn text(value: impl Into<String>) -> NonEmptyString {
    NonEmptyString::parse("value", value).unwrap()
}

fn limit(value: i64) -> PositiveLimit {
    PositiveLimit::new("limit", value).unwrap()
}

async fn server(pool: PgPool) -> (String, Client) {
    let root = std::env::temp_dir().join(format!("noctis-approvals-{}", Uuid::new_v4()));
    let app = Router::new()
        .merge(api::projects::router_with_scheduler(pool.clone(), None, 2))
        .merge(api::tasks::router(
            pool,
            ArtifactStore::new(&root, 1_048_576).unwrap(),
        ))
        .fallback(api::error::not_found)
        .layer(middleware::from_fn(api::request_id));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
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
    (
        response.status(),
        response.json().await.unwrap_or(Value::Null),
    )
}

async fn approvals(base: &str, client: &Client) -> Value {
    let response = client
        .get(format!("{base}/approvals"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

/// Project via API + run dengan budget tertentu (repository = direktori kerja ini, sudah repo Git).
async fn run_via_api(base: &str, client: &Client, budget: i64) -> (Uuid, Uuid) {
    let (project, run) = (Uuid::new_v4(), Uuid::new_v4());
    // Path repository harus unik per project; subdirektori repo ini juga repo Git yang valid.
    let path = std::fs::canonicalize(if budget == 50 {
        "."
    } else if budget == 1_000 {
        "tests"
    } else {
        "src"
    })
    .unwrap();
    let key = || Uuid::new_v4().to_string();
    send(
        client,
        Method::POST,
        &format!("{base}/projects"),
        &key(),
        json!({"id":project,"name":format!("Project {budget}"),"repository_path":path}),
    )
    .await;
    let (status, body) = send(client, Method::POST, &format!("{base}/projects/{project}/runs"), &key(),
        json!({"id":run,"project_id":project,"objective":format!("Objective {budget}"),"acceptance_criteria":["ok"],"token_budget":budget})).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (project, run)
}

fn plan_task(project: Uuid, run: Uuid) -> Value {
    json!({
        "id":Uuid::new_v4().to_string(),"project_id":project,"project_run_id":run,"title":"Ship","role":"worker","objective":"Ship",
        "depends_on":[],"allowed_paths":["src/**"],"context_refs":[],"acceptance_criteria":["ok"],"verification_commands":["cargo test"],
        "limits":{"max_input_tokens":20,"max_output_tokens":10,"max_tool_calls":1,"max_attempts":1,"timeout_seconds":10}
    })
}

#[sqlx::test(migrations = "./migrations")]
async fn plans_waiting_for_approval_show_risk_and_decisions_show_who_decided(pool: PgPool) {
    let (base, client) = server(pool).await;
    // Run kecil: reservasi plan (2 x 30 = 60) melebihi budget 50 -> over_budget. Run besar: cukup.
    let (small_project, small_run) = run_via_api(&base, &client, 50).await;
    let (big_project, big_run) = run_via_api(&base, &client, 1_000).await;
    for (project, run, id) in [
        (small_project, small_run, "plan-small"),
        (big_project, big_run, "plan-big"),
    ] {
        let (status, body) = send(&client, Method::POST, &format!("{base}/runs/{run}/plan"), &Uuid::new_v4().to_string(),
            json!({"id":id,"project_run_id":run,"version":1,"tasks":[plan_task(project, run), plan_task(project, run)],"risk_flags":["touches shared module"]})).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    let view = approvals(&base, &client).await;
    let pending: Vec<&Value> = view["pending_plans"].as_array().unwrap().iter().collect();
    assert_eq!(pending.len(), 2);
    let small = pending
        .iter()
        .find(|plan| plan["plan_id"] == "plan-small")
        .unwrap();
    assert_eq!(
        (
            small["task_count"].as_u64(),
            small["reserved_tokens"].as_i64(),
            small["available_tokens"].as_i64(),
            small["over_budget"].as_bool()
        ),
        (Some(2), Some(60), Some(50), Some(true))
    );
    assert_eq!(
        (
            small["project_name"].as_str(),
            small["run_objective"].as_str(),
            small["run_status"].as_str()
        ),
        (
            Some("Project 50"),
            Some("Objective 50"),
            Some("AWAITING_APPROVAL")
        )
    );
    assert_eq!(small["risk_flags"], json!(["touches shared module"]));
    let big = pending
        .iter()
        .find(|plan| plan["plan_id"] == "plan-big")
        .unwrap();
    assert_eq!(big["over_budget"], false);
    assert!(big["created_at"].as_str().unwrap().ends_with('Z'));
    assert!(view["plan_decisions"].as_array().unwrap().is_empty());

    // Keputusan tercatat beserta pelakunya; approve yang pasti gagal ditolak (409) dan plan tetap menunggu.
    let (status, _) = send(
        &client,
        Method::POST,
        &format!("{base}/runs/{small_run}/approve-plan"),
        "a1",
        json!({"plan_id":"plan-small","actor_id":"alice","decision":"APPROVED"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(send(&client, Method::POST, &format!("{base}/runs/{small_run}/reject-plan"), "a2", json!({"plan_id":"plan-small","actor_id":"bob","decision":"REJECTED","reason":"too big"})).await.0, StatusCode::OK);
    assert_eq!(send(&client, Method::POST, &format!("{base}/runs/{big_run}/approve-plan"), "a3", json!({"plan_id":"plan-big","actor_id":"alice","decision":"APPROVED","reason":"looks good"})).await.0, StatusCode::OK);
    // Plan yang sudah diputuskan = approval basi: ditolak.
    assert_eq!(
        send(
            &client,
            Method::POST,
            &format!("{base}/runs/{big_run}/approve-plan"),
            "a4",
            json!({"plan_id":"plan-big","actor_id":"mallory","decision":"APPROVED"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );

    let after = approvals(&base, &client).await;
    assert!(after["pending_plans"].as_array().unwrap().is_empty());
    let decisions: Vec<(String, String, String, Option<String>)> = after["plan_decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["plan_id"].as_str().unwrap().into(),
                d["outcome"].as_str().unwrap().into(),
                d["actor_id"].as_str().unwrap().into(),
                d["reason"].as_str().map(Into::into),
            )
        })
        .collect();
    assert!(decisions.contains(&(
        "plan-small".into(),
        "REJECTED".into(),
        "bob".into(),
        Some("too big".into())
    )));
    assert!(decisions.contains(&(
        "plan-big".into(),
        "APPROVED".into(),
        "alice".into(),
        Some("looks good".into())
    )));
    assert_eq!(
        decisions.len(),
        2,
        "keputusan yang ditolak karena basi tidak tercatat"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn human_attention_tasks_carry_context_and_decisions_are_audited(pool: PgPool) {
    let (base, client) = server(pool.clone()).await;
    let (project, run) = run_via_api(&base, &client, 10_000).await;
    let tasks = TaskRepository::new(pool.clone());
    for id in ["needs-human", "in-conflict", "fine"] {
        tasks
            .create(&TaskContract {
                id: text(id),
                project_id: text(project.to_string()),
                project_run_id: text(run.to_string()),
                title: text(format!("Title {id}")),
                role: text("worker"),
                objective: text("ship"),
                depends_on: vec![],
                allowed_paths: vec![AllowedPath::parse(format!("src/{id}/**")).unwrap()],
                context_refs: vec![],
                acceptance_criteria: vec![text("ok")],
                verification_commands: vec![text("cargo test")],
                limits: TaskLimits {
                    max_input_tokens: limit(10),
                    max_output_tokens: limit(10),
                    max_tool_calls: limit(1),
                    max_attempts: MaxAttempts::new(2).unwrap(),
                    timeout_seconds: limit(10),
                },
            })
            .await
            .unwrap();
    }
    sqlx::query("UPDATE tasks SET status='NEEDS_HUMAN' WHERE id='needs-human'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET status='CONFLICT' WHERE id='in-conflict'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET status='READY' WHERE id='fine'")
        .execute(&pool)
        .await
        .unwrap();
    let attempt = AgentAttempt {
        id: Uuid::new_v4(),
        task_id: text("needs-human"),
        role: text("worker"),
        attempt: 1,
        provider_id: text("p"),
        model_id: text("m"),
        status: text("recovery_required"),
    };
    tasks.create_attempt(&attempt).await.unwrap();
    sqlx::query("UPDATE agent_runs SET error_code='recovery.tool_in_progress',finished_at=now() WHERE id=$1").bind(attempt.id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO events (project_run_id,task_id,actor_type,event_type,payload) VALUES ($1,'needs-human','system','recovery','{\"disposition\":\"recovery_required\"}')").bind(run).execute(&pool).await.unwrap();

    let view = approvals(&base, &client).await;
    let items = view["attention_tasks"].as_array().unwrap();
    assert_eq!(items.len(), 2, "task READY tidak masuk antrean manusia");
    let human = items
        .iter()
        .find(|task| task["task_id"] == "needs-human")
        .unwrap();
    let conflict = items
        .iter()
        .find(|task| task["task_id"] == "in-conflict")
        .unwrap();
    assert_eq!(
        (
            human["can_retry"].as_bool(),
            conflict["can_retry"].as_bool()
        ),
        (Some(true), Some(false))
    );
    assert_eq!(
        (
            human["last_error"].as_str(),
            human["has_patch"].as_bool(),
            human["title"].as_str()
        ),
        (
            Some("recovery.tool_in_progress"),
            Some(false),
            Some("Title needs-human")
        )
    );
    assert_eq!(human["recent_events"][0]["event_type"], "recovery");
    assert_eq!(
        human["recent_events"][0]["payload"]["disposition"],
        "recovery_required"
    );
    assert_eq!(
        (
            human["project_name"].as_str(),
            human["run_objective"].as_str()
        ),
        (Some("Project 10000"), Some("Objective 10000"))
    );
    let version = human["version"].as_i64().unwrap();

    // Validasi identitas.
    let retry = format!("{base}/tasks/needs-human/retry");
    for (key, body) in [
        ("v1", json!({"expected_version":version,"actor_id":"  "})),
        (
            "v2",
            json!({"expected_version":version,"reason":"orphan reason"}),
        ),
        (
            "v3",
            json!({"expected_version":version,"actor_id":"a\u{7}b"}),
        ),
        (
            "v4",
            json!({"expected_version":version,"actor_id":"carol","reason":"x".repeat(501)}),
        ),
    ] {
        assert_eq!(
            send(&client, Method::POST, &retry, key, body).await.0,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{key}"
        );
    }
    // Tanpa identitas, NEEDS_HUMAN -> READY tetap ditolak (hanya manusia yang berhak).
    assert!(
        !send(
            &client,
            Method::POST,
            &retry,
            "anon",
            json!({"expected_version":version})
        )
        .await
        .0
        .is_success()
    );
    // Versi basi ditolak; versi benar + identitas diterima dan dicatat sebagai keputusan manusia.
    assert_eq!(
        send(
            &client,
            Method::POST,
            &retry,
            "stale",
            json!({"expected_version":version + 5,"actor_id":"carol"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (status, body) = send(
        &client,
        Method::POST,
        &retry,
        "ok",
        json!({"expected_version":version,"actor_id":"carol","reason":"upstream fixed"}),
    )
    .await;
    assert_eq!(
        (status, body["status"].as_str()),
        (StatusCode::OK, Some("READY"))
    );
    // Pembatalan task CONFLICT oleh manusia lain.
    let cancel = format!("{base}/tasks/in-conflict/cancel");
    let conflict_version = conflict["version"].as_i64().unwrap();
    assert_eq!(
        send(
            &client,
            Method::POST,
            &cancel,
            "cancel",
            json!({"expected_version":conflict_version,"actor_id":"dave","reason":"obsolete"})
        )
        .await
        .0,
        StatusCode::OK
    );

    let after = approvals(&base, &client).await;
    assert!(after["attention_tasks"].as_array().unwrap().is_empty());
    let decisions: Vec<(String, String, String, String, Option<String>)> = after["task_decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["task_id"].as_str().unwrap().into(),
                d["from_status"].as_str().unwrap().into(),
                d["to_status"].as_str().unwrap().into(),
                d["actor_id"].as_str().unwrap().into(),
                d["reason"].as_str().map(Into::into),
            )
        })
        .collect();
    assert!(decisions.contains(&(
        "needs-human".into(),
        "NEEDS_HUMAN".into(),
        "READY".into(),
        "carol".into(),
        Some("upstream fixed".into())
    )));
    assert!(decisions.contains(&(
        "in-conflict".into(),
        "CONFLICT".into(),
        "CANCELLED".into(),
        "dave".into(),
        Some("obsolete".into())
    )));
    assert_eq!(decisions.len(), 2);
    // Event task membawa identitas pelaku dan aktornya manusia.
    let events: Value = client
        .get(format!("{base}/tasks/needs-human/events"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let last = events["items"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(
        (
            last["actor"].as_str(),
            last["actor_id"].as_str(),
            last["to_status"].as_str()
        ),
        (Some("human"), Some("carol"), Some("READY"))
    );
    assert_eq!(last["payload"]["reason"], "upstream fixed");
}

#[sqlx::test(migrations = "./migrations")]
async fn empty_queue_is_well_formed_and_reading_changes_nothing(pool: PgPool) {
    let (base, client) = server(pool.clone()).await;
    let view = approvals(&base, &client).await;
    assert_eq!(
        view,
        json!({"pending_plans":[],"attention_tasks":[],"plan_decisions":[],"task_decisions":[]})
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}
