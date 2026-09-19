#[path = "../src/domain/provider.rs"]
mod provider;

use provider::{
    ClaimedCapabilities, EnvironmentVariable, HttpUrl, Model, ModelCapabilities, NonEmptyString,
    NonNegativeI64, PositiveI64, ProbeErrorCode, ProbeKind, ProbeResult, ProbeStatus, Provider,
    VerifiedCapabilities, VerifiedCapability,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

fn text(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}

fn parse<T: DeserializeOwned>(value: String) -> T {
    serde_json::from_value(Value::String(value)).unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn provider_model_and_probes_roundtrip_without_information_loss(pool: PgPool) {
    let large = i64::from(i32::MAX) + 1;
    let provider = Provider {
        id: NonEmptyString::parse("id", "primary").unwrap(),
        base_url: HttpUrl::parse("https://api.example.com/v1").unwrap(),
        api_key_env: EnvironmentVariable::parse("PRIMARY_API_KEY").unwrap(),
        request_timeout_seconds: PositiveI64::new("request_timeout_seconds", large).unwrap(),
    };
    let model = Model {
        id: NonEmptyString::parse("id", "coding-large").unwrap(),
        provider_id: NonEmptyString::parse("provider_id", provider.id.as_str()).unwrap(),
        remote_name: NonEmptyString::parse("remote_name", "vendor/coding-large").unwrap(),
        class: NonEmptyString::parse("class", "coding").unwrap(),
        context_window: PositiveI64::new("context_window", large).unwrap(),
        max_output_tokens: PositiveI64::new("max_output_tokens", large).unwrap(),
        capabilities: ModelCapabilities {
            claimed: ClaimedCapabilities {
                chat: true,
                streaming: true,
                tools: true,
                parallel_tools: false,
            },
            verified: VerifiedCapabilities::default(),
        },
    };
    assert_eq!(model.capabilities.verified, VerifiedCapabilities::default());

    sqlx::query(
        "INSERT INTO providers (id, base_url, api_key_env, request_timeout_seconds)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(provider.id.as_str())
    .bind(provider.base_url.as_str())
    .bind(provider.api_key_env.as_str())
    .bind(provider.request_timeout_seconds.get())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO models
         (id, provider_id, remote_name, class, context_window, max_output_tokens,
          claimed_capabilities, verified_capabilities)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(model.id.as_str())
    .bind(model.provider_id.as_str())
    .bind(model.remote_name.as_str())
    .bind(model.class.as_str())
    .bind(model.context_window.get())
    .bind(model.max_output_tokens.get())
    .bind(serde_json::to_value(&model.capabilities.claimed).unwrap())
    .bind(serde_json::to_value(&model.capabilities.verified).unwrap())
    .execute(&pool)
    .await
    .unwrap();

    let provider_row = sqlx::query(
        "SELECT id, base_url, api_key_env, request_timeout_seconds FROM providers WHERE id = $1",
    )
    .bind(provider.id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    let stored_provider = Provider {
        id: NonEmptyString::parse("id", provider_row.get::<String, _>("id")).unwrap(),
        base_url: HttpUrl::parse(provider_row.get::<String, _>("base_url")).unwrap(),
        api_key_env: EnvironmentVariable::parse(provider_row.get::<String, _>("api_key_env"))
            .unwrap(),
        request_timeout_seconds: PositiveI64::new(
            "request_timeout_seconds",
            provider_row.get("request_timeout_seconds"),
        )
        .unwrap(),
    };

    let model_row = sqlx::query(
        "SELECT id, provider_id, remote_name, class, context_window, max_output_tokens,
                claimed_capabilities, verified_capabilities
         FROM models WHERE id = $1",
    )
    .bind(model.id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    let stored_model = Model {
        id: NonEmptyString::parse("id", model_row.get::<String, _>("id")).unwrap(),
        provider_id: NonEmptyString::parse(
            "provider_id",
            model_row.get::<String, _>("provider_id"),
        )
        .unwrap(),
        remote_name: NonEmptyString::parse(
            "remote_name",
            model_row.get::<String, _>("remote_name"),
        )
        .unwrap(),
        class: NonEmptyString::parse("class", model_row.get::<String, _>("class")).unwrap(),
        context_window: PositiveI64::new("context_window", model_row.get("context_window"))
            .unwrap(),
        max_output_tokens: PositiveI64::new(
            "max_output_tokens",
            model_row.get("max_output_tokens"),
        )
        .unwrap(),
        capabilities: ModelCapabilities {
            claimed: serde_json::from_value(model_row.get("claimed_capabilities")).unwrap(),
            verified: serde_json::from_value(model_row.get("verified_capabilities")).unwrap(),
        },
    };
    assert_eq!(stored_provider, provider);
    assert_eq!(stored_model, model);

    let probes = [
        ProbeResult {
            provider_id: NonEmptyString::parse("provider_id", provider.id.as_str()).unwrap(),
            model_id: NonEmptyString::parse("model_id", model.id.as_str()).unwrap(),
            kind: ProbeKind::Chat,
            status: ProbeStatus::Succeeded,
            verified: VerifiedCapability::Supported,
            latency_ms: NonNegativeI64::new("latency_ms", 0).unwrap(),
            error_code: None,
        },
        ProbeResult {
            provider_id: NonEmptyString::parse("provider_id", provider.id.as_str()).unwrap(),
            model_id: NonEmptyString::parse("model_id", model.id.as_str()).unwrap(),
            kind: ProbeKind::Streaming,
            status: ProbeStatus::Failed,
            verified: VerifiedCapability::Unsupported,
            latency_ms: NonNegativeI64::new("latency_ms", 17).unwrap(),
            error_code: Some(ProbeErrorCode::Timeout),
        },
        ProbeResult {
            provider_id: NonEmptyString::parse("provider_id", provider.id.as_str()).unwrap(),
            model_id: NonEmptyString::parse("model_id", model.id.as_str()).unwrap(),
            kind: ProbeKind::Tools,
            status: ProbeStatus::Succeeded,
            verified: VerifiedCapability::Unknown,
            latency_ms: NonNegativeI64::new("latency_ms", large).unwrap(),
            error_code: Some(ProbeErrorCode::InvalidResponse),
        },
    ];

    for probe in &probes {
        sqlx::query(
            "INSERT INTO provider_probes
             (id, provider_id, model_id, probe_kind, status, capability_status,
              latency_ms, error_code)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(Uuid::new_v4())
        .bind(probe.provider_id.as_str())
        .bind(probe.model_id.as_str())
        .bind(text(&probe.kind))
        .bind(text(&probe.status))
        .bind(text(&probe.verified))
        .bind(probe.latency_ms.get())
        .bind(probe.error_code.as_ref().map(text))
        .execute(&pool)
        .await
        .unwrap();
    }

    let rows = sqlx::query(
        "SELECT provider_id, model_id, probe_kind, status, capability_status,
                latency_ms, error_code
         FROM provider_probes ORDER BY created_at, probe_kind",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let stored_probes: Vec<ProbeResult> = rows
        .into_iter()
        .map(|row| ProbeResult {
            provider_id: NonEmptyString::parse("provider_id", row.get::<String, _>("provider_id"))
                .unwrap(),
            model_id: NonEmptyString::parse("model_id", row.get::<String, _>("model_id")).unwrap(),
            kind: parse(row.get("probe_kind")),
            status: parse(row.get("status")),
            verified: parse(row.get("capability_status")),
            latency_ms: NonNegativeI64::new("latency_ms", row.get("latency_ms")).unwrap(),
            error_code: row.get::<Option<String>, _>("error_code").map(parse),
        })
        .collect();

    for probe in probes {
        assert!(stored_probes.contains(&probe), "missing probe {probe:?}");
    }
}
