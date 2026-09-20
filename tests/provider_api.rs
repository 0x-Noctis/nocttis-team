use std::net::SocketAddr;

use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    middleware,
    response::{IntoResponse, Response},
    routing::post,
};
use reqwest::Client;
use serde_json::{Value, json};
use sqlx::PgPool;

use ai_team::api;

struct Servers {
    api: String,
    provider: String,
}

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    address
}

async fn servers(pool: PgPool) -> Servers {
    unsafe { std::env::set_var("NOCTIS_PROVIDER_HOST_ALLOWLIST", "127.0.0.1,localhost") };
    let provider = serve(Router::new().route("/v1/chat/completions", post(mock_provider))).await;
    let api = api::providers::router(pool)
        .fallback(api::error::not_found)
        .layer(api::cors_layer("http://127.0.0.1:5173".parse().unwrap()))
        .layer(middleware::from_fn(api::request_id));
    let api = serve(api).await;
    Servers {
        api: format!("http://{api}"),
        provider: format!("http://{provider}/v1"),
    }
}

async fn mock_provider(headers: HeaderMap, Json(body): Json<Value>) -> Response {
    assert!(headers.get("authorization").is_some());
    if body.get("tools").is_some() {
        return Json(json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "probe-call",
                        "function": {
                            "name": "noctis_capability_probe",
                            "arguments": "{\"enabled\":true}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        }))
        .into_response();
    }
    if body["stream"] == true {
        return (
            [("content-type", "text/event-stream")],
            concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"OK\"},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\n",
                "data: [DONE]\n\n"
            ),
        )
            .into_response();
    }
    Json(json!({
        "choices": [{"message": {"content": "OK"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    }))
    .into_response()
}

fn provider(base_url: &str, id: &str) -> Value {
    json!({
        "id": id,
        "base_url": base_url,
        "api_key_env": "PATH",
        "request_timeout_seconds": 5
    })
}

fn model(provider_id: &str, id: &str) -> Value {
    json!({
        "id": id,
        "provider_id": provider_id,
        "remote_name": "mock/model",
        "class": "coding",
        "context_window": 4096,
        "max_output_tokens": 128,
        "claimed_capabilities": {
            "chat": true,
            "streaming": true,
            "tools": true,
            "parallel_tools": false
        }
    })
}

async fn mutation(
    client: &Client,
    method: reqwest::Method,
    url: &str,
    key: &str,
    body: Option<&Value>,
) -> reqwest::Response {
    let mut request = client.request(method, url).header("Idempotency-Key", key);
    if let Some(body) = body {
        request = request.json(body);
    }
    request.send().await.unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn provider_model_crud_idempotency_and_errors(pool: PgPool) {
    let servers = servers(pool.clone()).await;
    let client = Client::new();
    let providers = format!("{}/api/v1/providers", servers.api);

    let invalid_query = client
        .get(format!("{providers}?limit=invalid"))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid_query.status(), StatusCode::BAD_REQUEST);
    assert!(invalid_query.headers().contains_key("x-request-id"));
    assert!(invalid_query.json::<Value>().await.unwrap()["error"]["request_id"].is_string());

    let invalid_method = client.patch(&providers).send().await.unwrap();
    assert_eq!(invalid_method.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert!(invalid_method.headers().contains_key("x-request-id"));
    assert_eq!(
        invalid_method.json::<Value>().await.unwrap()["error"]["code"],
        "METHOD_NOT_ALLOWED"
    );

    let preflight = client
        .request(reqwest::Method::OPTIONS, &providers)
        .header("Origin", "http://127.0.0.1:5173")
        .header("Access-Control-Request-Method", "POST")
        .header(
            "Access-Control-Request-Headers",
            "content-type,idempotency-key",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(preflight.status(), StatusCode::OK);
    assert!(
        preflight
            .headers()
            .get("access-control-allow-headers")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("idempotency-key")
    );

    let missing_key = client
        .post(&providers)
        .json(&provider(&servers.provider, "primary"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    assert!(missing_key.headers().contains_key("x-request-id"));
    let missing_key_body: Value = missing_key.json().await.unwrap();
    assert_eq!(missing_key_body["error"]["code"], "BAD_REQUEST");
    assert!(missing_key_body["error"]["request_id"].is_string());

    let long_key = mutation(
        &client,
        reqwest::Method::POST,
        &providers,
        &"x".repeat(256),
        Some(&provider(&servers.provider, "primary")),
    )
    .await;
    assert_eq!(long_key.status(), StatusCode::BAD_REQUEST);

    let created = mutation(
        &client,
        reqwest::Method::POST,
        &providers,
        "provider-create",
        Some(&provider(&servers.provider, "primary")),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created_body: Value = created.json().await.unwrap();
    assert_eq!(created_body["secret_configured"], true);
    assert!(created_body.get("api_key").is_none());

    let replay = mutation(
        &client,
        reqwest::Method::POST,
        &providers,
        "provider-create",
        Some(&provider(&servers.provider, "primary")),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::CREATED);
    assert_eq!(replay.json::<Value>().await.unwrap(), created_body);

    let conflict = mutation(
        &client,
        reqwest::Method::POST,
        &providers,
        "provider-create",
        Some(&provider(&servers.provider, "other")),
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let duplicate = mutation(
        &client,
        reqwest::Method::POST,
        &providers,
        "provider-duplicate",
        Some(&provider(&servers.provider, "primary")),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);

    let unknown = json!({
        "id": "bad",
        "base_url": servers.provider,
        "api_key_env": "PATH",
        "request_timeout_seconds": 5,
        "api_key": "must-not-be-accepted"
    });
    let invalid = mutation(
        &client,
        reqwest::Method::POST,
        &providers,
        "provider-invalid",
        Some(&unknown),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let list: Value = client
        .get(&providers)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    let get: Value = client
        .get(format!("{providers}/primary"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(get["id"], "primary");

    let mut updated = provider(&servers.provider, "body-id-is-ignored");
    updated["request_timeout_seconds"] = json!(8);
    let update = mutation(
        &client,
        reqwest::Method::PUT,
        &format!("{providers}/primary"),
        "provider-update",
        Some(&updated),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    assert_eq!(update.json::<Value>().await.unwrap()["id"], "primary");

    let models = format!("{providers}/primary/models");
    let model_created = mutation(
        &client,
        reqwest::Method::POST,
        &models,
        "model-create",
        Some(&model("body-provider-ignored", "model-a")),
    )
    .await;
    assert_eq!(model_created.status(), StatusCode::CREATED);
    let model_body: Value = model_created.json().await.unwrap();
    assert_eq!(model_body["provider_id"], "primary");

    let models_list: Value = client
        .get(&models)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(models_list["items"].as_array().unwrap().len(), 1);
    let model_url = format!("{}/api/v1/models/model-a", servers.api);
    assert_eq!(
        client.get(&model_url).send().await.unwrap().status(),
        StatusCode::OK
    );

    let mut model_update = model("primary", "ignored");
    model_update["max_output_tokens"] = json!(256);
    let model_updated = mutation(
        &client,
        reqwest::Method::PUT,
        &model_url,
        "model-update",
        Some(&model_update),
    )
    .await;
    assert_eq!(model_updated.status(), StatusCode::OK);
    assert_eq!(
        model_updated.json::<Value>().await.unwrap()["id"],
        "model-a"
    );

    let model_deleted = mutation(
        &client,
        reqwest::Method::DELETE,
        &model_url,
        "model-delete",
        None,
    )
    .await;
    assert_eq!(model_deleted.status(), StatusCode::OK);
    let deleted_body: Value = model_deleted.json().await.unwrap();
    assert_eq!(deleted_body, json!({"deleted": true, "id": "model-a"}));
    let delete_replay = mutation(
        &client,
        reqwest::Method::DELETE,
        &model_url,
        "model-delete",
        None,
    )
    .await;
    assert_eq!(delete_replay.json::<Value>().await.unwrap(), deleted_body);

    let provider_deleted = mutation(
        &client,
        reqwest::Method::DELETE,
        &format!("{providers}/primary"),
        "provider-delete",
        None,
    )
    .await;
    assert_eq!(provider_deleted.status(), StatusCode::OK);
    assert_eq!(
        client
            .get(format!("{providers}/primary"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn normal_streaming_and_tool_probes_are_persisted(pool: PgPool) {
    let servers = servers(pool.clone()).await;
    let client = Client::new();
    let providers = format!("{}/api/v1/providers", servers.api);
    mutation(
        &client,
        reqwest::Method::POST,
        &providers,
        "probe-provider",
        Some(&provider(&servers.provider, "primary")),
    )
    .await;
    mutation(
        &client,
        reqwest::Method::POST,
        &format!("{providers}/primary/models"),
        "probe-model",
        Some(&model("primary", "model-a")),
    )
    .await;

    for kind in ["chat", "streaming", "tools"] {
        let response = mutation(
            &client,
            reqwest::Method::POST,
            &format!("{}/api/v1/models/model-a/probes/{kind}", servers.api),
            &format!("probe-{kind}"),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{kind}");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["result"]["verified"], "supported", "{kind}");
    }

    let probes: i64 = sqlx::query_scalar("SELECT count(*) FROM provider_probes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(probes, 3);
    let verified: Value =
        sqlx::query_scalar("SELECT verified_capabilities FROM models WHERE id = 'model-a'")
            .fetch_one(&pool)
            .await
            .unwrap();
    for kind in ["chat", "streaming", "tools"] {
        assert_eq!(verified[kind], "supported");
    }
}
