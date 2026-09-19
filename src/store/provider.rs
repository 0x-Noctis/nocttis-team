use std::{env, error::Error, fmt};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use uuid::Uuid;

use crate::domain::provider::{
    ClaimedCapabilities, EnvironmentVariable, HttpUrl, Model, ModelCapabilities, NonEmptyString,
    NonNegativeI64, PositiveI64, ProbeErrorCode, ProbeKind, ProbeResult, Provider, ValidationError,
    VerifiedCapabilities,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    ProviderId,
    ModelId,
    RemoteName,
}

#[derive(Debug)]
pub enum StoreError {
    Conflict(Conflict),
    NotFound,
    InvalidRow(ValidationError),
    InvalidRowJson(serde_json::Error),
    Database(sqlx::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(conflict) => write!(formatter, "conflict: {conflict:?}"),
            Self::NotFound => formatter.write_str("resource not found"),
            Self::InvalidRow(error) => write!(formatter, "invalid database row: {error}"),
            Self::InvalidRowJson(_) => formatter.write_str("invalid JSON in database row"),
            Self::Database(_) => formatter.write_str("database operation failed"),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidRow(error) => Some(error),
            Self::InvalidRowJson(error) => Some(error),
            Self::Database(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ValidationError> for StoreError {
    fn from(error: ValidationError) -> Self {
        Self::InvalidRow(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::InvalidRowJson(error)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderView {
    pub provider: Provider,
    pub secret_configured: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Clone)]
pub struct ProviderRepository {
    pool: PgPool,
}

impl ProviderRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create_provider(&self, provider: &Provider) -> Result<ProviderView, StoreError> {
        sqlx::query(
            "INSERT INTO providers (id, base_url, api_key_env, request_timeout_seconds)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(provider.id.as_str())
        .bind(provider.base_url.as_str())
        .bind(provider.api_key_env.as_str())
        .bind(provider.request_timeout_seconds.get())
        .execute(&self.pool)
        .await
        .map_err(|error| map_database_error(error, Conflict::ProviderId))?;
        Ok(provider_view(provider.clone()))
    }

    pub async fn get_provider(&self, id: &str) -> Result<ProviderView, StoreError> {
        let row = sqlx::query(
            "SELECT id, base_url, api_key_env, request_timeout_seconds
             FROM providers WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(StoreError::Database)?
        .ok_or(StoreError::NotFound)?;
        Ok(provider_view(provider_from_row(&row)?))
    }

    pub async fn list_providers(
        &self,
        after_id: Option<&str>,
        limit: i64,
    ) -> Result<Page<ProviderView>, StoreError> {
        let limit = validated_limit(limit)?;
        let rows = sqlx::query(
            "SELECT id, base_url, api_key_env, request_timeout_seconds
             FROM providers
             WHERE ($1::text IS NULL OR id > $1)
             ORDER BY id
             LIMIT $2",
        )
        .bind(after_id)
        .bind(limit + 1)
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        page(rows, limit, |row| provider_from_row(row).map(provider_view))
    }

    pub async fn update_provider(&self, provider: &Provider) -> Result<ProviderView, StoreError> {
        let result = sqlx::query(
            "UPDATE providers
             SET base_url = $2, api_key_env = $3, request_timeout_seconds = $4, updated_at = now()
             WHERE id = $1",
        )
        .bind(provider.id.as_str())
        .bind(provider.base_url.as_str())
        .bind(provider.api_key_env.as_str())
        .bind(provider.request_timeout_seconds.get())
        .execute(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(provider_view(provider.clone()))
    }

    pub async fn delete_provider(&self, id: &str) -> Result<(), StoreError> {
        let result = sqlx::query("DELETE FROM providers WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    pub async fn create_model(&self, model: &Model) -> Result<Model, StoreError> {
        write_model(&self.pool, model, false).await?;
        Ok(model.clone())
    }

    pub async fn get_model(&self, id: &str) -> Result<Model, StoreError> {
        let row = sqlx::query(
            "SELECT id, provider_id, remote_name, class, context_window, max_output_tokens,
                    claimed_capabilities, verified_capabilities
             FROM models WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(StoreError::Database)?
        .ok_or(StoreError::NotFound)?;
        model_from_row(&row)
    }

    pub async fn list_models(
        &self,
        provider_id: &str,
        after_id: Option<&str>,
        limit: i64,
    ) -> Result<Page<Model>, StoreError> {
        let limit = validated_limit(limit)?;
        let rows = sqlx::query(
            "SELECT id, provider_id, remote_name, class, context_window, max_output_tokens,
                    claimed_capabilities, verified_capabilities
             FROM models
             WHERE provider_id = $1 AND ($2::text IS NULL OR id > $2)
             ORDER BY id
             LIMIT $3",
        )
        .bind(provider_id)
        .bind(after_id)
        .bind(limit + 1)
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        page(rows, limit, model_from_row)
    }

    pub async fn update_model(&self, model: &Model) -> Result<Model, StoreError> {
        write_model(&self.pool, model, true).await?;
        Ok(model.clone())
    }

    pub async fn delete_model(&self, id: &str) -> Result<(), StoreError> {
        let result = sqlx::query("DELETE FROM models WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    pub async fn save_probe(&self, probe: &ProbeResult) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await.map_err(StoreError::Database)?;
        sqlx::query(
            "INSERT INTO provider_probes
             (id, provider_id, model_id, probe_kind, status, capability_status, latency_ms, error_code)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(Uuid::new_v4())
        .bind(probe.provider_id.as_str())
        .bind(probe.model_id.as_str())
        .bind(enum_text(&probe.kind)?)
        .bind(enum_text(&probe.status)?)
        .bind(enum_text(&probe.verified)?)
        .bind(probe.latency_ms.get())
        .bind(probe.error_code.as_ref().map(enum_text).transpose()?)
        .execute(&mut *transaction)
        .await
        .map_err(StoreError::Database)?;
        update_verified_capability(&mut transaction, probe).await?;
        transaction.commit().await.map_err(StoreError::Database)
    }

    pub async fn list_probes(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<Vec<ProbeResult>, StoreError> {
        let rows = sqlx::query(
            "SELECT provider_id, model_id, probe_kind, status, capability_status,
                    latency_ms, error_code
             FROM provider_probes
             WHERE provider_id = $1 AND model_id = $2
             ORDER BY created_at, id",
        )
        .bind(provider_id)
        .bind(model_id)
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        rows.iter().map(probe_from_row).collect()
    }
}

async fn write_model(pool: &PgPool, model: &Model, update: bool) -> Result<(), StoreError> {
    let claimed = serde_json::to_value(&model.capabilities.claimed)?;
    let verified = serde_json::to_value(&model.capabilities.verified)?;
    let result = if update {
        sqlx::query(
            "UPDATE models
             SET provider_id = $2, remote_name = $3, class = $4, context_window = $5,
                 max_output_tokens = $6, claimed_capabilities = $7,
                 verified_capabilities = $8, updated_at = now()
             WHERE id = $1",
        )
        .bind(model.id.as_str())
        .bind(model.provider_id.as_str())
        .bind(model.remote_name.as_str())
        .bind(model.class.as_str())
        .bind(model.context_window.get())
        .bind(model.max_output_tokens.get())
        .bind(claimed)
        .bind(verified)
        .execute(pool)
        .await
    } else {
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
        .bind(claimed)
        .bind(verified)
        .execute(pool)
        .await
    };
    match result {
        Ok(result) if update && result.rows_affected() == 0 => Err(StoreError::NotFound),
        Ok(_) => Ok(()),
        Err(error) => Err(map_model_database_error(error)),
    }
}

async fn update_verified_capability(
    transaction: &mut Transaction<'_, Postgres>,
    probe: &ProbeResult,
) -> Result<(), StoreError> {
    let path = match probe.kind {
        ProbeKind::Chat => "chat",
        ProbeKind::Streaming => "streaming",
        ProbeKind::Tools => "tools",
    };
    let result = sqlx::query(
        "UPDATE models
         SET verified_capabilities = jsonb_set(verified_capabilities, ARRAY[$3], to_jsonb($4::text)),
             updated_at = now()
         WHERE provider_id = $1 AND id = $2",
    )
    .bind(probe.provider_id.as_str())
    .bind(probe.model_id.as_str())
    .bind(path)
    .bind(enum_text(&probe.verified)?)
    .execute(&mut **transaction)
    .await
    .map_err(StoreError::Database)?;
    if result.rows_affected() == 0 {
        return Err(StoreError::NotFound);
    }
    Ok(())
}

fn provider_view(provider: Provider) -> ProviderView {
    let secret_configured = env::var_os(provider.api_key_env.as_str()).is_some();
    ProviderView {
        provider,
        secret_configured,
    }
}

fn provider_from_row(row: &PgRow) -> Result<Provider, StoreError> {
    Ok(Provider {
        id: NonEmptyString::parse(
            "id",
            row.try_get::<String, _>("id")
                .map_err(StoreError::Database)?,
        )?,
        base_url: HttpUrl::parse(
            row.try_get::<String, _>("base_url")
                .map_err(StoreError::Database)?,
        )?,
        api_key_env: EnvironmentVariable::parse(
            row.try_get::<String, _>("api_key_env")
                .map_err(StoreError::Database)?,
        )?,
        request_timeout_seconds: PositiveI64::new(
            "request_timeout_seconds",
            row.try_get("request_timeout_seconds")
                .map_err(StoreError::Database)?,
        )?,
    })
}

fn model_from_row(row: &PgRow) -> Result<Model, StoreError> {
    Ok(Model {
        id: NonEmptyString::parse(
            "id",
            row.try_get::<String, _>("id")
                .map_err(StoreError::Database)?,
        )?,
        provider_id: NonEmptyString::parse(
            "provider_id",
            row.try_get::<String, _>("provider_id")
                .map_err(StoreError::Database)?,
        )?,
        remote_name: NonEmptyString::parse(
            "remote_name",
            row.try_get::<String, _>("remote_name")
                .map_err(StoreError::Database)?,
        )?,
        class: NonEmptyString::parse(
            "class",
            row.try_get::<String, _>("class")
                .map_err(StoreError::Database)?,
        )?,
        context_window: PositiveI64::new(
            "context_window",
            row.try_get("context_window")
                .map_err(StoreError::Database)?,
        )?,
        max_output_tokens: PositiveI64::new(
            "max_output_tokens",
            row.try_get("max_output_tokens")
                .map_err(StoreError::Database)?,
        )?,
        capabilities: ModelCapabilities {
            claimed: serde_json::from_value::<ClaimedCapabilities>(
                row.try_get::<Value, _>("claimed_capabilities")
                    .map_err(StoreError::Database)?,
            )?,
            verified: serde_json::from_value::<VerifiedCapabilities>(
                row.try_get::<Value, _>("verified_capabilities")
                    .map_err(StoreError::Database)?,
            )?,
        },
    })
}

fn probe_from_row(row: &PgRow) -> Result<ProbeResult, StoreError> {
    Ok(ProbeResult {
        provider_id: NonEmptyString::parse(
            "provider_id",
            row.try_get::<String, _>("provider_id")
                .map_err(StoreError::Database)?,
        )?,
        model_id: NonEmptyString::parse(
            "model_id",
            row.try_get::<String, _>("model_id")
                .map_err(StoreError::Database)?,
        )?,
        kind: parse_enum(row.try_get("probe_kind").map_err(StoreError::Database)?)?,
        status: parse_enum(row.try_get("status").map_err(StoreError::Database)?)?,
        verified: parse_enum(
            row.try_get("capability_status")
                .map_err(StoreError::Database)?,
        )?,
        latency_ms: NonNegativeI64::new(
            "latency_ms",
            row.try_get("latency_ms").map_err(StoreError::Database)?,
        )?,
        error_code: row
            .try_get::<Option<String>, _>("error_code")
            .map_err(StoreError::Database)?
            .map(parse_enum::<ProbeErrorCode>)
            .transpose()?,
    })
}

fn enum_text(value: &impl Serialize) -> Result<String, StoreError> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| {
            StoreError::InvalidRowJson(serde_json::Error::io(std::io::Error::other(
                "enum must serialize as string",
            )))
        })
}

fn parse_enum<T: DeserializeOwned>(value: String) -> Result<T, StoreError> {
    Ok(serde_json::from_value(Value::String(value))?)
}

fn validated_limit(limit: i64) -> Result<i64, StoreError> {
    if limit <= 0 {
        return Err(StoreError::InvalidRow(ValidationError {
            field: "limit",
            message: "must be positive",
        }));
    }
    Ok(limit.min(100))
}

fn page<T>(
    mut rows: Vec<PgRow>,
    limit: i64,
    map: impl Fn(&PgRow) -> Result<T, StoreError>,
) -> Result<Page<T>, StoreError> {
    let has_more = rows.len() > limit as usize;
    if has_more {
        rows.pop();
    }
    let next_cursor = has_more
        .then(|| {
            rows.last()
                .and_then(|row| row.try_get::<String, _>("id").ok())
        })
        .flatten();
    Ok(Page {
        items: rows.iter().map(map).collect::<Result<_, _>>()?,
        next_cursor,
    })
}

fn map_database_error(error: sqlx::Error, conflict: Conflict) -> StoreError {
    if error
        .as_database_error()
        .and_then(|database| database.constraint())
        .is_some()
    {
        StoreError::Conflict(conflict)
    } else {
        StoreError::Database(error)
    }
}

fn map_model_database_error(error: sqlx::Error) -> StoreError {
    let conflict = match error
        .as_database_error()
        .and_then(|database| database.constraint())
    {
        Some("models_pkey" | "models_provider_id_id_key") => Some(Conflict::ModelId),
        Some("models_provider_id_remote_name_key") => Some(Conflict::RemoteName),
        _ => None,
    };
    conflict
        .map(StoreError::Conflict)
        .unwrap_or(StoreError::Database(error))
}
