use ai_team::api;
use axum::{Router, middleware};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::net::TcpListener;
use uuid::Uuid;

async fn server(pool: PgPool) -> (String, Client, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .merge(api::projects::router(pool))
        .fallback(api::error::not_found)
        .layer(middleware::from_fn(api::request_id));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/v1", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, Client::new(), handle)
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
    let status = response.status();
    let body = response.json().await.unwrap();
    (status, body)
}

fn task(project: Uuid, run: Uuid) -> Value {
    json!({
        "id":Uuid::new_v4().to_string(), "project_id":project, "project_run_id":run,
        "title":"Ship", "role":"worker", "objective":"Ship", "depends_on":[],
        "allowed_paths":["src/**"], "context_refs":[], "acceptance_criteria":["Tests pass"],
        "verification_commands":["cargo test"],
        "limits":{"max_input_tokens":20,"max_output_tokens":10,"max_tool_calls":1,"max_attempts":1,"timeout_seconds":10}
    })
}

#[sqlx::test(migrations = "./migrations")]
async fn project_lifecycle_requires_human_decision_and_idempotent_mutations(pool: PgPool) {
    let (base, client, handle) = server(pool.clone()).await;
    let project = Uuid::new_v4();
    let run = Uuid::new_v4();
    let path = std::fs::canonicalize(".")
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let create = json!({"id":project,"name":"Project","repository_path":path});
    let url = format!("{base}/projects");
    let (created, first) = send(
        &client,
        Method::POST,
        &url,
        "create-project",
        create.clone(),
    )
    .await;
    assert_eq!(created, StatusCode::CREATED, "{first}");
    assert_eq!(
        send(
            &client,
            Method::POST,
            &url,
            "create-project",
            create.clone()
        )
        .await,
        (created, first)
    );
    assert_eq!(
        send(
            &client,
            Method::POST,
            &url,
            "create-project",
            json!({"id":Uuid::new_v4(),"name":"Other","repository_path":path})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        client
            .get(format!("{url}/{project}"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        client.get(&url).send().await.unwrap().status(),
        StatusCode::OK
    );
    let discover_url = format!("{url}/{project}/discover");
    let (status, discovered) =
        send(&client, Method::POST, &discover_url, "discover", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{discovered}");
    assert!(
        !discovered["repository_map"]["files"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let run_url = format!("{url}/{project}/runs");
    let (status, _) = send(&client, Method::POST, &run_url, "bad-run", json!({"id":Uuid::new_v4(),"project_id":project,"objective":" ","acceptance_criteria":[],"token_budget":0})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = send(&client, Method::POST, &run_url, "run", json!({"id":run,"project_id":project,"objective":"Ship","acceptance_criteria":["Tests pass"],"token_budget":100})).await;
    assert_eq!(status, StatusCode::CREATED);
    let plan = format!("{base}/runs/{run}/plan");
    let plan_id = format!("plan-{run}");
    let proposal = json!({"id":plan_id,"project_run_id":run,"version":1,"tasks":[task(project,run)],"risk_flags":[]});
    let (status, proposed) = send(&client, Method::POST, &plan, "proposal", proposal).await;
    assert_eq!(status, StatusCode::CREATED, "{proposed}");
    assert_eq!(proposed["plan"]["status"], "PROPOSED");
    let status_url = format!("{base}/runs/{run}");
    assert_eq!(
        client
            .get(&status_url)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()["run"]["status"],
        "AWAITING_APPROVAL"
    );
    let pause = format!("{status_url}/pause");
    assert_eq!(
        send(
            &client,
            Method::POST,
            &pause,
            "early-pause",
            json!({"expected_status":"RUNNING"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let approve = format!("{status_url}/approve-plan");
    let (status, _) = send(
        &client,
        Method::POST,
        &approve,
        "no-human",
        json!({"plan_id":plan_id,"actor_id":" ","decision":"APPROVED"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let approval = json!({"plan_id":plan_id,"actor_id":"human","decision":"APPROVED"});
    let (status, approved) = send(
        &client,
        Method::POST,
        &approve,
        "approval",
        approval.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["plan"]["status"], "APPROVED");
    assert_eq!(
        send(&client, Method::POST, &approve, "approval", approval).await,
        (status, approved)
    );
    assert_eq!(
        send(
            &client,
            Method::POST,
            &approve,
            "second-approval",
            json!({"plan_id":plan_id,"actor_id":"human","decision":"APPROVED"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tasks WHERE project_run_id=$1")
            .bind(run)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        send(
            &client,
            Method::POST,
            &pause,
            "pause",
            json!({"expected_status":"RUNNING"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(
            &client,
            Method::POST,
            &pause,
            "stale-pause",
            json!({"expected_status":"RUNNING"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(
            &client,
            Method::POST,
            &format!("{status_url}/resume"),
            "resume",
            json!({"expected_status":"PAUSED"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(
            &client,
            Method::POST,
            &format!("{status_url}/cancel"),
            "cancel",
            json!({"expected_status":"RUNNING"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(
            &client,
            Method::DELETE,
            &format!("{url}/{project}"),
            "delete-active",
            json!({"expected_name":"Project","expected_repository_path":path})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    handle.abort();
}

#[sqlx::test(migrations = "./migrations")]
async fn reject_and_safe_delete(pool: PgPool) {
    let (base, client, handle) = server(pool).await;
    let project = Uuid::new_v4();
    let path = std::fs::canonicalize(".")
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let url = format!("{base}/projects");
    assert_eq!(
        send(
            &client,
            Method::POST,
            &url,
            "create",
            json!({"id":project,"name":"Project","repository_path":path})
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let update_url = format!("{url}/{project}");
    assert_eq!(send(&client, Method::PUT, &update_url, "stale", json!({"id":project,"name":"Edited","repository_path":path,"expected_name":"Old","expected_repository_path":path})).await.0, StatusCode::CONFLICT);
    assert_eq!(send(&client, Method::PUT, &update_url, "update", json!({"id":project,"name":"Edited","repository_path":path,"expected_name":"Project","expected_repository_path":path})).await.0, StatusCode::OK);
    let run = Uuid::new_v4();
    assert_eq!(send(&client, Method::POST, &format!("{update_url}/runs"), "run", json!({"id":run,"project_id":project,"objective":"Ship","acceptance_criteria":["ok"],"token_budget":100})).await.0, StatusCode::CREATED);
    let plan_id = format!("plan-{run}");
    assert_eq!(
        send(
            &client,
            Method::POST,
            &format!("{base}/runs/{run}/plan"),
            "plan",
            json!({"id":plan_id,"project_run_id":run,"version":1,"tasks":[task(project,run)]})
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        send(
            &client,
            Method::POST,
            &format!("{base}/runs/{run}/reject-plan"),
            "reject-no-reason",
            json!({"plan_id":plan_id,"actor_id":"human","decision":"REJECTED"})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        send(
            &client,
            Method::POST,
            &format!("{base}/runs/{run}/reject-plan"),
            "reject",
            json!({"plan_id":plan_id,"actor_id":"human","decision":"REJECTED","reason":"revise"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(
            &client,
            Method::POST,
            &format!("{base}/runs/{run}/approve-plan"),
            "late",
            json!({"plan_id":plan_id,"actor_id":"human","decision":"APPROVED"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(
            &client,
            Method::DELETE,
            &update_url,
            "delete-run",
            json!({"expected_name":"Edited","expected_repository_path":path})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    handle.abort();
}

#[sqlx::test(migrations = "./migrations")]
async fn mutation_boundaries_and_paused_cancel(pool: PgPool) {
    let (base, client, handle) = server(pool.clone()).await;
    let url = format!("{base}/projects");
    let path = std::fs::canonicalize(".")
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let project = Uuid::new_v4();
    let payload = json!({"id":project,"name":"Project","repository_path":path});
    let response = client.post(&url).json(&payload).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"]["code"],
        "BAD_REQUEST"
    );
    assert_eq!(
        send(
            &client,
            Method::POST,
            &url,
            "bad-path",
            json!({"id":Uuid::new_v4(),"name":"Bad","repository_path":"/not-a-repository"})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        send(&client, Method::POST, &url, "project", payload)
            .await
            .0,
        StatusCode::CREATED
    );
    let run = Uuid::new_v4();
    let run_url = format!("{url}/{project}/runs");
    let run_payload = json!({"id":run,"project_id":project,"objective":"Ship","acceptance_criteria":["Tests"],"token_budget":100});
    let (status, first) = send(
        &client,
        Method::POST,
        &run_url,
        "create-run",
        run_payload.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    assert_eq!(
        send(&client, Method::POST, &run_url, "create-run", run_payload).await,
        (status, first)
    );
    let plan_id = format!("plan-{run}");
    let plan_url = format!("{base}/runs/{run}/plan");
    let plan = json!({"id":plan_id,"project_run_id":run,"version":1,"tasks":[task(project,run)]});
    assert_eq!(
        send(&client, Method::POST, &plan_url, "plan", plan.clone())
            .await
            .0,
        StatusCode::CREATED
    );
    assert_eq!(
        send(&client, Method::POST, &plan_url, "plan", plan).await.0,
        StatusCode::CREATED
    );
    assert_eq!(
        send(
            &client,
            Method::POST,
            &format!("{base}/runs/{run}/reject-plan"),
            "wrong-decision",
            json!({"plan_id":plan_id,"actor_id":"human","decision":"APPROVED"})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        send(
            &client,
            Method::POST,
            &format!("{base}/runs/{run}/approve-plan"),
            "approve",
            json!({"plan_id":plan_id,"actor_id":"human","decision":"APPROVED"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let status_url = format!("{base}/runs/{run}");
    assert_eq!(
        send(
            &client,
            Method::POST,
            &format!("{status_url}/pause"),
            "pause",
            json!({"expected_status":"RUNNING"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let cancel_url = format!("{status_url}/cancel");
    let (status, result) = send(
        &client,
        Method::POST,
        &cancel_url,
        "cancel",
        json!({"expected_status":"PAUSED"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["run"]["status"], "CANCELLED");
    assert_eq!(
        send(
            &client,
            Method::POST,
            &cancel_url,
            "cancel",
            json!({"expected_status":"PAUSED"})
        )
        .await,
        (status, result)
    );
    assert_eq!(
        send(
            &client,
            Method::POST,
            &format!("{status_url}/resume"),
            "resume",
            json!({"expected_status":"PAUSED"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tasks WHERE project_run_id=$1")
            .bind(run)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    handle.abort();
}
