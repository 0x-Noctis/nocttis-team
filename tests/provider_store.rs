use ai_team::domain;
#[path = "../src/store/provider.rs"]
mod provider_store;

use domain::provider::{
    ClaimedCapabilities, EnvironmentVariable, HttpUrl, MAX_SAFE_INTEGER, Model, ModelCapabilities,
    NonEmptyString, NonNegativeI64, PositiveI64, ProbeErrorCode, ProbeKind, ProbeResult,
    ProbeStatus, Provider, VerifiedCapabilities, VerifiedCapability,
};
use provider_store::{Conflict, ProviderRepository, StoreError};
use sqlx::{PgPool, Row};

fn provider(id: &str, api_key_env: &str) -> Provider {
    Provider {
        id: NonEmptyString::parse("id", id).unwrap(),
        base_url: HttpUrl::parse(format!("https://{id}.example.com/v1")).unwrap(),
        api_key_env: EnvironmentVariable::parse(api_key_env).unwrap(),
        request_timeout_seconds: PositiveI64::new("request_timeout_seconds", 180).unwrap(),
    }
}

fn model(id: &str, provider_id: &str, remote_name: &str) -> Model {
    Model {
        id: NonEmptyString::parse("id", id).unwrap(),
        provider_id: NonEmptyString::parse("provider_id", provider_id).unwrap(),
        remote_name: NonEmptyString::parse("remote_name", remote_name).unwrap(),
        class: NonEmptyString::parse("class", "coding").unwrap(),
        context_window: PositiveI64::new("context_window", 131_072).unwrap(),
        max_output_tokens: PositiveI64::new("max_output_tokens", 16_384).unwrap(),
        capabilities: ModelCapabilities {
            claimed: ClaimedCapabilities {
                chat: true,
                streaming: true,
                tools: true,
                parallel_tools: false,
            },
            verified: VerifiedCapabilities::default(),
        },
    }
}

fn probe(
    provider_id: &str,
    model_id: &str,
    kind: ProbeKind,
    verified: VerifiedCapability,
) -> ProbeResult {
    ProbeResult {
        provider_id: NonEmptyString::parse("provider_id", provider_id).unwrap(),
        model_id: NonEmptyString::parse("model_id", model_id).unwrap(),
        kind,
        status: ProbeStatus::Succeeded,
        verified,
        latency_ms: NonNegativeI64::new("latency_ms", 0).unwrap(),
        error_code: None,
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn provider_crud_pagination_and_secret_status(pool: PgPool) {
    let repository = ProviderRepository::new(pool);
    let configured = provider("provider-b", "PATH");
    let missing = provider("provider-a", "NOCTIS_TEST_SECRET_MISSING_7DDB75C4");

    assert!(
        repository
            .create_provider(&configured)
            .await
            .unwrap()
            .secret_configured
    );
    assert!(
        !repository
            .create_provider(&missing)
            .await
            .unwrap()
            .secret_configured
    );
    assert_eq!(
        repository
            .get_provider("provider-b")
            .await
            .unwrap()
            .provider,
        configured
    );

    let first = repository.list_providers(None, 1).await.unwrap();
    assert_eq!(first.items[0].provider.id.as_str(), "provider-a");
    assert_eq!(first.next_cursor.as_deref(), Some("provider-a"));
    let second = repository
        .list_providers(first.next_cursor.as_deref(), 1)
        .await
        .unwrap();
    assert_eq!(second.items[0].provider.id.as_str(), "provider-b");
    assert!(second.next_cursor.is_none());

    let mut updated = configured.clone();
    updated.base_url = HttpUrl::parse("http://localhost:8080/v1").unwrap();
    updated.request_timeout_seconds = PositiveI64::new("request_timeout_seconds", 30).unwrap();
    assert_eq!(
        repository.update_provider(&updated).await.unwrap().provider,
        updated
    );
    repository.delete_provider("provider-b").await.unwrap();
    assert!(matches!(
        repository.get_provider("provider-b").await,
        Err(StoreError::NotFound)
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn provider_and_model_conflicts_are_structured(pool: PgPool) {
    let repository = ProviderRepository::new(pool);
    for value in [
        provider("provider-a", "KEY_A"),
        provider("provider-b", "KEY_B"),
    ] {
        repository.create_provider(&value).await.unwrap();
    }
    assert!(matches!(
        repository
            .create_provider(&provider("provider-a", "OTHER_KEY"))
            .await,
        Err(StoreError::Conflict(Conflict::ProviderId))
    ));

    repository
        .create_model(&model("model-a", "provider-a", "vendor/shared"))
        .await
        .unwrap();
    assert!(matches!(
        repository
            .create_model(&model("model-a", "provider-b", "vendor/other"))
            .await,
        Err(StoreError::Conflict(Conflict::ModelId))
    ));
    assert!(matches!(
        repository
            .create_model(&model("model-b", "provider-a", "vendor/shared"))
            .await,
        Err(StoreError::Conflict(Conflict::RemoteName))
    ));
    repository
        .create_model(&model("model-b", "provider-b", "vendor/shared"))
        .await
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn provider_check_constraint_is_not_mapped_to_duplicate_id(pool: PgPool) {
    let repository = ProviderRepository::new(pool.clone());
    let invalid = Provider {
        request_timeout_seconds: PositiveI64::new("request_timeout_seconds", 1).unwrap(),
        ..provider("invalid", "VALID_ENV")
    };
    sqlx::query(
        "ALTER TABLE providers ADD CONSTRAINT providers_timeout_test_check
         CHECK (request_timeout_seconds > 10)",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        repository.create_provider(&invalid).await,
        Err(StoreError::Database(_))
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn model_crud_probe_roundtrip_and_cascade(pool: PgPool) {
    let repository = ProviderRepository::new(pool.clone());
    repository
        .create_provider(&provider("primary", "PRIMARY_API_KEY"))
        .await
        .unwrap();
    let created = model("model-a", "primary", "vendor/model-a");
    repository.create_model(&created).await.unwrap();
    assert_eq!(repository.get_model("model-a").await.unwrap(), created);

    let mut updated = created.clone();
    updated.remote_name = NonEmptyString::parse("remote_name", "vendor/model-b").unwrap();
    updated.context_window = PositiveI64::new("context_window", 262_144).unwrap();
    assert_eq!(repository.update_model(&updated).await.unwrap(), updated);

    repository
        .create_model(&model("model-b", "primary", "vendor/model-c"))
        .await
        .unwrap();
    let first = repository.list_models("primary", None, 1).await.unwrap();
    assert_eq!(first.items[0].id.as_str(), "model-a");
    let second = repository
        .list_models("primary", first.next_cursor.as_deref(), 1)
        .await
        .unwrap();
    assert_eq!(second.items[0].id.as_str(), "model-b");

    let probes = [
        probe(
            "primary",
            "model-a",
            ProbeKind::Chat,
            VerifiedCapability::Supported,
        ),
        probe(
            "primary",
            "model-a",
            ProbeKind::Streaming,
            VerifiedCapability::Unsupported,
        ),
        ProbeResult {
            status: ProbeStatus::Failed,
            error_code: Some(ProbeErrorCode::Timeout),
            ..probe(
                "primary",
                "model-a",
                ProbeKind::Tools,
                VerifiedCapability::Unknown,
            )
        },
    ];
    for value in &probes {
        repository.save_probe(value).await.unwrap();
    }
    let stored = repository.list_probes("primary", "model-a").await.unwrap();
    for value in probes {
        assert!(stored.contains(&value));
    }
    let verified = repository
        .get_model("model-a")
        .await
        .unwrap()
        .capabilities
        .verified;
    assert_eq!(verified.chat, VerifiedCapability::Supported);
    assert_eq!(verified.streaming, VerifiedCapability::Unsupported);
    assert_eq!(verified.tools, VerifiedCapability::Unknown);

    repository.delete_model("model-b").await.unwrap();
    assert!(matches!(
        repository.get_model("model-b").await,
        Err(StoreError::NotFound)
    ));
    repository.delete_provider("primary").await.unwrap();
    let model_count: i64 = sqlx::query("SELECT count(*) AS count FROM models")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("count");
    let probe_count: i64 = sqlx::query("SELECT count(*) AS count FROM provider_probes")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("count");
    assert_eq!((model_count, probe_count), (0, 0));
}

#[sqlx::test(migrations = "./migrations")]
async fn invalid_database_row_is_rejected_by_domain_validation(pool: PgPool) {
    let repository = ProviderRepository::new(pool.clone());
    repository
        .create_provider(&provider("primary", "PRIMARY_API_KEY"))
        .await
        .unwrap();
    repository
        .create_model(&model("model-a", "primary", "vendor/model"))
        .await
        .unwrap();

    let constraints: Vec<String> = sqlx::query(
        "SELECT conname FROM pg_constraint
         WHERE conrelid = 'models'::regclass AND contype = 'c'",
    )
    .fetch_all(&pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| row.get("conname"))
    .collect();
    for constraint in constraints {
        let statement = format!("ALTER TABLE models DROP CONSTRAINT {constraint}");
        sqlx::query(&statement).execute(&pool).await.unwrap();
    }
    sqlx::query("UPDATE models SET context_window = $1 WHERE id = 'model-a'")
        .bind(MAX_SAFE_INTEGER + 1)
        .execute(&pool)
        .await
        .unwrap();

    assert!(matches!(
        repository.get_model("model-a").await,
        Err(StoreError::InvalidRow(_))
    ));
}
