use serde_json::json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

async fn insert_provider(pool: &PgPool, provider_id: &str, timeout: i64) {
    insert_provider_with_url(pool, provider_id, "https://api.example.com/v1", timeout).await;
}

async fn insert_provider_with_url(pool: &PgPool, provider_id: &str, base_url: &str, timeout: i64) {
    sqlx::query(
        "INSERT INTO providers (id, base_url, api_key_env, request_timeout_seconds)
         VALUES ($1, $2, 'EXAMPLE_API_KEY', $3)",
    )
    .bind(provider_id)
    .bind(base_url)
    .bind(timeout)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_model(
    pool: &PgPool,
    provider_id: &str,
    model_id: &str,
    remote_name: &str,
    context_window: i64,
    max_output_tokens: i64,
) {
    sqlx::query(
        "INSERT INTO models
         (id, provider_id, remote_name, class, context_window, max_output_tokens)
         VALUES ($1, $2, $3, 'coding', $4, $5)",
    )
    .bind(model_id)
    .bind(provider_id)
    .bind(remote_name)
    .bind(context_window)
    .bind(max_output_tokens)
    .execute(pool)
    .await
    .unwrap();
}

fn database_code(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()
        .unwrap()
        .code()
        .map(|code| code.into_owned())
}

#[sqlx::test(migrations = "./migrations")]
async fn text_ids_and_bigint_values_are_supported(pool: PgPool) {
    let large = i64::from(i32::MAX) + 1;
    insert_provider(&pool, "provider/openai-compatible", large).await;
    insert_model(
        &pool,
        "provider/openai-compatible",
        "model/coding-large",
        "vendor/coding-large",
        large,
        large,
    )
    .await;

    let row = sqlx::query(
        "SELECT p.id AS provider_id, m.id AS model_id, p.request_timeout_seconds,
                m.context_window, m.max_output_tokens
         FROM providers p JOIN models m ON m.provider_id = p.id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row.get::<String, _>("provider_id"),
        "provider/openai-compatible"
    );
    assert_eq!(row.get::<String, _>("model_id"), "model/coding-large");
    assert_eq!(row.get::<i64, _>("request_timeout_seconds"), large);
    assert_eq!(row.get::<i64, _>("context_window"), large);
    assert_eq!(row.get::<i64, _>("max_output_tokens"), large);
}

#[sqlx::test(migrations = "./migrations")]
async fn non_positive_numeric_values_are_rejected(pool: PgPool) {
    for timeout in [0_i64, -1] {
        let error = sqlx::query(
            "INSERT INTO providers (id, base_url, api_key_env, request_timeout_seconds)
             VALUES ($1, 'https://api.example.com/v1', 'KEY_ENV', $2)",
        )
        .bind(format!("invalid-timeout-{timeout}"))
        .bind(timeout)
        .execute(&pool)
        .await
        .unwrap_err();
        assert_eq!(database_code(&error).as_deref(), Some("23514"));
    }

    insert_provider(&pool, "numeric-provider", 180).await;
    for (context_window, max_output_tokens) in [(0_i64, 1_i64), (-1, 1), (1, 0), (1, -1)] {
        let error = sqlx::query(
            "INSERT INTO models
             (id, provider_id, remote_name, class, context_window, max_output_tokens)
             VALUES ($1, 'numeric-provider', $1, 'coding', $2, $3)",
        )
        .bind(format!("invalid-{context_window}-{max_output_tokens}"))
        .bind(context_window)
        .bind(max_output_tokens)
        .execute(&pool)
        .await
        .unwrap_err();
        assert_eq!(database_code(&error).as_deref(), Some("23514"));
    }

    insert_model(
        &pool,
        "numeric-provider",
        "valid-model",
        "valid-model",
        1,
        1,
    )
    .await;
    let error = sqlx::query(
        "INSERT INTO provider_probes
         (id, provider_id, model_id, probe_kind, status, latency_ms)
         VALUES ($1, 'numeric-provider', 'valid-model', 'chat', 'failed', -1)",
    )
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(database_code(&error).as_deref(), Some("23514"));
}

#[sqlx::test(migrations = "./migrations")]
async fn capability_states_are_lossless_and_validated(pool: PgPool) {
    insert_provider(&pool, "capability-provider", 180).await;
    insert_model(
        &pool,
        "capability-provider",
        "capability-model",
        "vendor/capability-model",
        131072,
        16384,
    )
    .await;

    let defaults = sqlx::query(
        "SELECT claimed_capabilities, verified_capabilities
         FROM models WHERE id = 'capability-model'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        defaults.get::<serde_json::Value, _>("claimed_capabilities"),
        json!({"chat": false, "streaming": false, "tools": false, "parallel_tools": false})
    );
    assert_eq!(
        defaults.get::<serde_json::Value, _>("verified_capabilities"),
        json!({
            "chat": "unknown",
            "streaming": "unknown",
            "tools": "unknown",
            "parallel_tools": "unknown"
        })
    );

    sqlx::query(
        "UPDATE models SET claimed_capabilities = $1, verified_capabilities = $2
         WHERE id = 'capability-model'",
    )
    .bind(json!({
        "chat": true,
        "streaming": false,
        "tools": true,
        "parallel_tools": false
    }))
    .bind(json!({
        "chat": "supported",
        "streaming": "unsupported",
        "tools": "unknown",
        "parallel_tools": "supported"
    }))
    .execute(&pool)
    .await
    .unwrap();

    for (probe_kind, capability_status) in [
        ("chat", "supported"),
        ("streaming", "unsupported"),
        ("tools", "unknown"),
    ] {
        sqlx::query(
            "INSERT INTO provider_probes
             (id, provider_id, model_id, probe_kind, status, capability_status, latency_ms)
             VALUES ($1, 'capability-provider', 'capability-model', $2, 'succeeded', $3, 0)",
        )
        .bind(Uuid::new_v4())
        .bind(probe_kind)
        .bind(capability_status)
        .execute(&pool)
        .await
        .unwrap();
    }

    let states: Vec<String> = sqlx::query_scalar(
        "SELECT capability_status FROM provider_probes ORDER BY capability_status",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(states, ["supported", "unknown", "unsupported"]);

    let invalid_claim =
        sqlx::query("UPDATE models SET claimed_capabilities = $1 WHERE id = 'capability-model'")
            .bind(json!({
                "chat": "yes",
                "streaming": false,
                "tools": true,
                "parallel_tools": false
            }))
            .execute(&pool)
            .await
            .unwrap_err();
    assert_eq!(database_code(&invalid_claim).as_deref(), Some("23514"));

    let invalid_verified =
        sqlx::query("UPDATE models SET verified_capabilities = $1 WHERE id = 'capability-model'")
            .bind(json!({
                "chat": "maybe",
                "streaming": "unknown",
                "tools": "unknown",
                "parallel_tools": "unknown"
            }))
            .execute(&pool)
            .await
            .unwrap_err();
    assert_eq!(database_code(&invalid_verified).as_deref(), Some("23514"));

    for claimed in [
        json!({"chat": true, "streaming": false, "tools": true}),
        json!({
            "chat": true,
            "streaming": false,
            "tools": true,
            "parallel_tools": false,
            "extra": false
        }),
    ] {
        let error = sqlx::query(
            "UPDATE models SET claimed_capabilities = $1 WHERE id = 'capability-model'",
        )
        .bind(claimed)
        .execute(&pool)
        .await
        .unwrap_err();
        assert_eq!(database_code(&error).as_deref(), Some("23514"));
    }

    for verified in [
        json!({"chat": "unknown", "streaming": "unknown", "tools": "unknown"}),
        json!({
            "chat": "unknown",
            "streaming": "unknown",
            "tools": "unknown",
            "parallel_tools": "unknown",
            "extra": "unknown"
        }),
    ] {
        let error = sqlx::query(
            "UPDATE models SET verified_capabilities = $1 WHERE id = 'capability-model'",
        )
        .bind(verified)
        .execute(&pool)
        .await
        .unwrap_err();
        assert_eq!(database_code(&error).as_deref(), Some("23514"));
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn provider_url_requires_normal_http_host_without_userinfo(pool: PgPool) {
    insert_provider_with_url(&pool, "http-provider", "http://localhost:8080/v1", 180).await;
    insert_provider_with_url(&pool, "https-provider", "https://api.example.com/v1", 180).await;

    for (provider_id, base_url) in [
        ("empty-host", "https:///v1"),
        ("userinfo", "https://user:password@example.com/v1"),
        ("wrong-scheme", "ftp://api.example.com/v1"),
    ] {
        let error = sqlx::query(
            "INSERT INTO providers (id, base_url, api_key_env, request_timeout_seconds)
             VALUES ($1, $2, 'EXAMPLE_API_KEY', 180)",
        )
        .bind(provider_id)
        .bind(base_url)
        .execute(&pool)
        .await
        .unwrap_err();
        assert_eq!(database_code(&error).as_deref(), Some("23514"));
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn probe_latency_is_required_and_zero_is_valid(pool: PgPool) {
    insert_provider(&pool, "latency-provider", 180).await;
    insert_model(
        &pool,
        "latency-provider",
        "latency-model",
        "vendor/latency",
        8192,
        1024,
    )
    .await;

    let null_latency = sqlx::query(
        "INSERT INTO provider_probes
         (id, provider_id, model_id, probe_kind, status, latency_ms)
         VALUES ($1, 'latency-provider', 'latency-model', 'chat', 'failed', NULL)",
    )
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(database_code(&null_latency).as_deref(), Some("23502"));

    sqlx::query(
        "INSERT INTO provider_probes
         (id, provider_id, model_id, probe_kind, status, latency_ms)
         VALUES ($1, 'latency-provider', 'latency-model', 'chat', 'succeeded', 0)",
    )
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn probe_error_codes_are_normalized(pool: PgPool) {
    insert_provider(&pool, "error-provider", 180).await;
    insert_model(
        &pool,
        "error-provider",
        "error-model",
        "vendor/error",
        8192,
        1024,
    )
    .await;

    for error_code in [
        "authentication_failed",
        "rate_limited",
        "timeout",
        "invalid_response",
        "provider_unavailable",
        "context_too_large",
    ] {
        sqlx::query(
            "INSERT INTO provider_probes
             (id, provider_id, model_id, probe_kind, status, latency_ms, error_code)
             VALUES ($1, 'error-provider', 'error-model', 'chat', 'failed', 0, $2)",
        )
        .bind(Uuid::new_v4())
        .bind(error_code)
        .execute(&pool)
        .await
        .unwrap();
    }

    let arbitrary = sqlx::query(
        "INSERT INTO provider_probes
         (id, provider_id, model_id, probe_kind, status, latency_ms, error_code)
         VALUES ($1, 'error-provider', 'error-model', 'chat', 'failed', 0, 'vendor_error')",
    )
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(database_code(&arbitrary).as_deref(), Some("23514"));
}

#[sqlx::test(migrations = "./migrations")]
async fn remote_name_is_unique_only_within_provider(pool: PgPool) {
    insert_provider(&pool, "first-provider", 180).await;
    insert_provider(&pool, "second-provider", 180).await;
    insert_model(
        &pool,
        "first-provider",
        "first-model",
        "vendor/shared",
        8192,
        1024,
    )
    .await;

    let duplicate = sqlx::query(
        "INSERT INTO models
         (id, provider_id, remote_name, class, context_window, max_output_tokens)
         VALUES ('duplicate-model', 'first-provider', 'vendor/shared', 'coding', 8192, 1024)",
    )
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(database_code(&duplicate).as_deref(), Some("23505"));

    insert_model(
        &pool,
        "second-provider",
        "second-model",
        "vendor/shared",
        8192,
        1024,
    )
    .await;
}

#[sqlx::test(migrations = "./migrations")]
async fn cross_provider_probe_is_rejected(pool: PgPool) {
    insert_provider(&pool, "model-owner", 180).await;
    insert_provider(&pool, "probe-owner", 180).await;
    insert_model(
        &pool,
        "model-owner",
        "owned-model",
        "vendor/owned",
        8192,
        1024,
    )
    .await;

    let mismatch = sqlx::query(
        "INSERT INTO provider_probes
         (id, provider_id, model_id, probe_kind, status, latency_ms)
         VALUES ($1, 'probe-owner', 'owned-model', 'chat', 'succeeded', 0)",
    )
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(database_code(&mismatch).as_deref(), Some("23503"));
}

#[sqlx::test(migrations = "./migrations")]
async fn probe_kind_and_status_are_restricted(pool: PgPool) {
    insert_provider(&pool, "probe-provider", 180).await;
    insert_model(
        &pool,
        "probe-provider",
        "probe-model",
        "vendor/probe",
        8192,
        1024,
    )
    .await;

    for (probe_kind, status) in [("embeddings", "succeeded"), ("chat", "pending")] {
        let error = sqlx::query(
            "INSERT INTO provider_probes
             (id, provider_id, model_id, probe_kind, status, latency_ms)
             VALUES ($1, 'probe-provider', 'probe-model', $2, $3, 0)",
        )
        .bind(Uuid::new_v4())
        .bind(probe_kind)
        .bind(status)
        .execute(&pool)
        .await
        .unwrap_err();
        assert_eq!(database_code(&error).as_deref(), Some("23514"));
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn deleting_provider_cascades_models_and_probes(pool: PgPool) {
    insert_provider(&pool, "cascade-provider", 180).await;
    insert_model(
        &pool,
        "cascade-provider",
        "cascade-model",
        "vendor/cascade",
        8192,
        1024,
    )
    .await;
    sqlx::query(
        "INSERT INTO provider_probes
         (id, provider_id, model_id, probe_kind, status, capability_status, latency_ms, error_code)
         VALUES ($1, 'cascade-provider', 'cascade-model', 'tools', 'failed',
                 'unsupported', 25, 'invalid_response')",
    )
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query("DELETE FROM providers WHERE id = 'cascade-provider'")
        .execute(&pool)
        .await
        .unwrap();

    let model_count: i64 = sqlx::query_scalar("SELECT count(*) FROM models")
        .fetch_one(&pool)
        .await
        .unwrap();
    let probe_count: i64 = sqlx::query_scalar("SELECT count(*) FROM provider_probes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((model_count, probe_count), (0, 0));
}

#[sqlx::test(migrations = "./migrations")]
async fn registry_has_no_secret_or_provider_body_columns(pool: PgPool) {
    let forbidden_columns: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM information_schema.columns
         WHERE table_schema = 'public'
           AND table_name IN ('providers', 'models', 'provider_probes')
           AND column_name IN (
               'api_key', 'secret', 'secret_value', 'credential', 'token',
               'provider_message', 'provider_body', 'response_body'
           )",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(forbidden_columns, 0);
}
