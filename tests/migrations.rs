use serde_json::json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

async fn insert_provider(pool: &PgPool) -> Uuid {
    let provider_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO providers
         (id, name, base_url, api_key_env, request_timeout_seconds)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(provider_id)
    .bind(format!("provider-{provider_id}"))
    .bind("https://api.example.com/v1")
    .bind("EXAMPLE_API_KEY")
    .bind(180_i32)
    .execute(pool)
    .await
    .unwrap();
    provider_id
}

async fn insert_model(pool: &PgPool, provider_id: Uuid, remote_name: &str) -> Uuid {
    let model_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO models
         (id, provider_id, name, remote_name, class, context_window, max_output_tokens,
          claimed_capabilities, verified_capabilities)
         VALUES ($1, $2, $3, $4, 'coding', 131072, 16384, $5, $6)",
    )
    .bind(model_id)
    .bind(provider_id)
    .bind(format!("model-{model_id}"))
    .bind(remote_name)
    .bind(json!({"streaming": true, "tools": true}))
    .bind(json!({"streaming": true, "tools": false}))
    .execute(pool)
    .await
    .unwrap();
    model_id
}

#[sqlx::test(migrations = "./migrations")]
async fn provider_registry_stores_capabilities_without_secret_values(pool: PgPool) {
    let provider_id = insert_provider(&pool).await;
    let model_id = insert_model(&pool, provider_id, "vendor/coding-large").await;

    let capabilities =
        sqlx::query("SELECT claimed_capabilities, verified_capabilities FROM models WHERE id = $1")
            .bind(model_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        capabilities.get::<serde_json::Value, _>("claimed_capabilities"),
        json!({"streaming": true, "tools": true})
    );
    assert_eq!(
        capabilities.get::<serde_json::Value, _>("verified_capabilities"),
        json!({"streaming": true, "tools": false})
    );

    let forbidden_columns: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM information_schema.columns
         WHERE table_schema = 'public'
           AND table_name IN ('providers', 'models', 'provider_probes')
           AND column_name IN ('api_key', 'secret', 'secret_value', 'credential', 'token')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(forbidden_columns, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn remote_model_name_is_unique_per_provider(pool: PgPool) {
    let first_provider = insert_provider(&pool).await;
    let second_provider = insert_provider(&pool).await;
    insert_model(&pool, first_provider, "vendor/shared-name").await;

    let duplicate = sqlx::query(
        "INSERT INTO models
         (id, provider_id, name, remote_name, class, context_window, max_output_tokens)
         VALUES ($1, $2, 'duplicate', 'vendor/shared-name', 'coding', 8192, 1024)",
    )
    .bind(Uuid::new_v4())
    .bind(first_provider)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(
        duplicate.as_database_error().unwrap().code().as_deref(),
        Some("23505")
    );

    insert_model(&pool, second_provider, "vendor/shared-name").await;
}

#[sqlx::test(migrations = "./migrations")]
async fn deleting_provider_cascades_models_and_probes(pool: PgPool) {
    let provider_id = insert_provider(&pool).await;
    let model_id = insert_model(&pool, provider_id, "vendor/cascade").await;
    sqlx::query(
        "INSERT INTO provider_probes
         (id, provider_id, model_id, probe_type, status, verified_capabilities, latency_ms)
         VALUES ($1, $2, $3, 'compatibility', 'passed', $4, 25)",
    )
    .bind(Uuid::new_v4())
    .bind(provider_id)
    .bind(model_id)
    .bind(json!({"streaming": true}))
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query("DELETE FROM providers WHERE id = $1")
        .bind(provider_id)
        .execute(&pool)
        .await
        .unwrap();

    let model_count: i64 = sqlx::query_scalar("SELECT count(*) FROM models WHERE provider_id = $1")
        .bind(provider_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let probe_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM provider_probes WHERE provider_id = $1")
            .bind(provider_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((model_count, probe_count), (0, 0));
}

#[sqlx::test(migrations = "./migrations")]
async fn probe_model_must_belong_to_provider(pool: PgPool) {
    let first_provider = insert_provider(&pool).await;
    let second_provider = insert_provider(&pool).await;
    let model_id = insert_model(&pool, first_provider, "vendor/owned-model").await;

    let mismatch = sqlx::query(
        "INSERT INTO provider_probes (id, provider_id, model_id, probe_type, status)
         VALUES ($1, $2, $3, 'compatibility', 'passed')",
    )
    .bind(Uuid::new_v4())
    .bind(second_provider)
    .bind(model_id)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(
        mismatch.as_database_error().unwrap().code().as_deref(),
        Some("23503")
    );
}
