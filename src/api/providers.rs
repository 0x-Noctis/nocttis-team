use std::{env, future::Future, net::IpAddr, time::Duration};

use crate::{
    model::{Message, MessageRole, ModelErrorKind, ModelLimits, ModelRequest},
    openai::{OpenAiChatClient, OpenAiStreamClient, OpenAiToolsClient, ToolProbeResult},
};
use axum::{
    Json, Router,
    extract::{
        Extension, Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, Method, StatusCode, header::HeaderName},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;

use crate::{
    domain::provider::{
        Model, NonNegativeI64, ProbeErrorCode, ProbeKind, ProbeResult, ProbeStatus, Provider,
        VerifiedCapability,
    },
    store::{
        idempotency::{IdempotencyRepository, Reservation},
        provider::{Conflict, ProviderRepository, StoreError},
    },
};

use super::{
    AppError, RequestId,
    contracts::provider::{
        ModelInput, ModelResponse, ProbeResponse, ProviderInput, ProviderResponse,
    },
};

const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");

#[derive(Clone)]
struct ApiState {
    providers: ProviderRepository,
    idempotency: IdempotencyRepository,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pagination {
    cursor: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
}

fn default_limit() -> i64 {
    50
}

pub fn router(pool: PgPool) -> Router {
    let state = ApiState {
        providers: ProviderRepository::new(pool.clone()),
        idempotency: IdempotencyRepository::new(pool),
    };
    Router::new()
        .route(
            "/api/v1/providers",
            get(list_providers).post(create_provider),
        )
        .route(
            "/api/v1/providers/{provider_id}",
            get(get_provider)
                .put(update_provider)
                .delete(delete_provider),
        )
        .route(
            "/api/v1/providers/{provider_id}/models",
            get(list_models).post(create_model),
        )
        .route(
            "/api/v1/models/{model_id}",
            get(get_model).put(update_model).delete(delete_model),
        )
        .route("/api/v1/models/{model_id}/probes/{kind}", post(probe_model))
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(state)
}

async fn list_providers(
    State(state): State<ApiState>,
    Extension(request_id): Extension<RequestId>,
    page: Result<Query<Pagination>, QueryRejection>,
) -> Result<Json<Value>, AppError> {
    let Query(page) = page
        .map_err(|_| AppError::bad_request(request_id, json!({"query": "invalid pagination"})))?;
    let page = state
        .providers
        .list_providers(page.cursor.as_deref(), page.limit)
        .await
        .map_err(|error| store_error(error, request_id))?;
    Ok(Json(json!({
        "items": page.items.into_iter().map(|view| ProviderResponse::from_domain(&view.provider, view.secret_configured)).collect::<Vec<_>>(),
        "next_cursor": page.next_cursor,
    })))
}

async fn create_provider(
    State(state): State<ApiState>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    let body = body(payload, request_id)?;
    let provider = Provider::try_from(parse::<ProviderInput>(&body, request_id)?)
        .map_err(|error| validation(error.field, error.message, request_id))?;
    mutate(
        &state,
        &headers,
        Method::POST,
        "/api/v1/providers",
        &body,
        request_id,
        || async {
            let view = state
                .providers
                .create_provider(&provider)
                .await
                .map_err(|error| store_error(error, request_id))?;
            response(
                StatusCode::CREATED,
                ProviderResponse::from_domain(&view.provider, view.secret_configured),
                request_id,
            )
        },
    )
    .await
}

async fn get_provider(
    State(state): State<ApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(provider_id): Path<String>,
) -> Result<Json<ProviderResponse>, AppError> {
    let view = state
        .providers
        .get_provider(&provider_id)
        .await
        .map_err(|error| store_error(error, request_id))?;
    Ok(Json(ProviderResponse::from_domain(
        &view.provider,
        view.secret_configured,
    )))
}

async fn update_provider(
    State(state): State<ApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(provider_id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    let body = body(payload, request_id)?;
    let mut input = parse::<ProviderInput>(&body, request_id)?;
    input.id = provider_id.clone();
    let provider = Provider::try_from(input)
        .map_err(|error| validation(error.field, error.message, request_id))?;
    let route = format!("/api/v1/providers/{provider_id}");
    mutate(
        &state,
        &headers,
        Method::PUT,
        &route,
        &body,
        request_id,
        || async {
            let view = state
                .providers
                .update_provider(&provider)
                .await
                .map_err(|error| store_error(error, request_id))?;
            response(
                StatusCode::OK,
                ProviderResponse::from_domain(&view.provider, view.secret_configured),
                request_id,
            )
        },
    )
    .await
}

async fn delete_provider(
    State(state): State<ApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(provider_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let route = format!("/api/v1/providers/{provider_id}");
    mutate(
        &state,
        &headers,
        Method::DELETE,
        &route,
        &json!({}),
        request_id,
        || async {
            state
                .providers
                .delete_provider(&provider_id)
                .await
                .map_err(|error| store_error(error, request_id))?;
            response(
                StatusCode::OK,
                json!({"deleted": true, "id": provider_id}),
                request_id,
            )
        },
    )
    .await
}

async fn list_models(
    State(state): State<ApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(provider_id): Path<String>,
    page: Result<Query<Pagination>, QueryRejection>,
) -> Result<Json<Value>, AppError> {
    let Query(page) = page
        .map_err(|_| AppError::bad_request(request_id, json!({"query": "invalid pagination"})))?;
    state
        .providers
        .get_provider(&provider_id)
        .await
        .map_err(|error| store_error(error, request_id))?;
    let page = state
        .providers
        .list_models(&provider_id, page.cursor.as_deref(), page.limit)
        .await
        .map_err(|error| store_error(error, request_id))?;
    Ok(Json(json!({
        "items": page.items.iter().map(ModelResponse::from).collect::<Vec<_>>(),
        "next_cursor": page.next_cursor,
    })))
}

async fn create_model(
    State(state): State<ApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(provider_id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    let body = body(payload, request_id)?;
    let mut input = parse::<ModelInput>(&body, request_id)?;
    input.provider_id = provider_id.clone();
    let model = Model::try_from(input)
        .map_err(|error| validation(error.field, error.message, request_id))?;
    let route = format!("/api/v1/providers/{provider_id}/models");
    mutate(
        &state,
        &headers,
        Method::POST,
        &route,
        &body,
        request_id,
        || async {
            let model = state
                .providers
                .create_model(&model)
                .await
                .map_err(|error| store_error(error, request_id))?;
            response(StatusCode::CREATED, ModelResponse::from(&model), request_id)
        },
    )
    .await
}

async fn get_model(
    State(state): State<ApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(model_id): Path<String>,
) -> Result<Json<ModelResponse>, AppError> {
    let model = state
        .providers
        .get_model(&model_id)
        .await
        .map_err(|error| store_error(error, request_id))?;
    Ok(Json(ModelResponse::from(&model)))
}

async fn update_model(
    State(state): State<ApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(model_id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    let body = body(payload, request_id)?;
    let mut input = parse::<ModelInput>(&body, request_id)?;
    input.id = model_id.clone();
    let model = Model::try_from(input)
        .map_err(|error| validation(error.field, error.message, request_id))?;
    let route = format!("/api/v1/models/{model_id}");
    mutate(
        &state,
        &headers,
        Method::PUT,
        &route,
        &body,
        request_id,
        || async {
            let model = state
                .providers
                .update_model(&model)
                .await
                .map_err(|error| store_error(error, request_id))?;
            response(StatusCode::OK, ModelResponse::from(&model), request_id)
        },
    )
    .await
}

async fn delete_model(
    State(state): State<ApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(model_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let route = format!("/api/v1/models/{model_id}");
    mutate(
        &state,
        &headers,
        Method::DELETE,
        &route,
        &json!({}),
        request_id,
        || async {
            state
                .providers
                .delete_model(&model_id)
                .await
                .map_err(|error| store_error(error, request_id))?;
            response(
                StatusCode::OK,
                json!({"deleted": true, "id": model_id}),
                request_id,
            )
        },
    )
    .await
}

async fn probe_model(
    State(state): State<ApiState>,
    Extension(request_id): Extension<RequestId>,
    Path((model_id, kind)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let kind = match kind.as_str() {
        "chat" => ProbeKind::Chat,
        "streaming" => ProbeKind::Streaming,
        "tools" => ProbeKind::Tools,
        _ => {
            return Err(validation(
                "kind",
                "must be chat, streaming, or tools",
                request_id,
            ));
        }
    };
    let route = format!("/api/v1/models/{model_id}/probes/{}", kind_name(kind));
    mutate(
        &state,
        &headers,
        Method::POST,
        &route,
        &json!({}),
        request_id,
        || async {
            let model = state
                .providers
                .get_model(&model_id)
                .await
                .map_err(|error| store_error(error, request_id))?;
            let provider = state
                .providers
                .get_provider(model.provider_id.as_str())
                .await
                .map_err(|error| store_error(error, request_id))?
                .provider;
            let probe = run_probe(&provider, &model, kind).await;
            state
                .providers
                .save_probe(&probe)
                .await
                .map_err(|error| store_error(error, request_id))?;
            if probe.error_code == Some(ProbeErrorCode::RateLimited) {
                return Err(AppError::rate_limited(
                    request_id,
                    json!({"model_id": model_id}),
                ));
            }
            response(StatusCode::OK, ProbeResponse { result: probe }, request_id)
        },
    )
    .await
}

async fn run_probe(provider: &Provider, model: &Model, kind: ProbeKind) -> ProbeResult {
    if !provider_destination_allowed(provider.base_url.as_str()).await {
        return failed_probe(
            provider,
            model,
            kind,
            ModelErrorKind::ProviderUnavailable,
            0,
        );
    }
    let api_key = match env::var(provider.api_key_env.as_str()) {
        Ok(value) => value,
        Err(_) => {
            return failed_probe(
                provider,
                model,
                kind,
                ModelErrorKind::AuthenticationFailed,
                0,
            );
        }
    };
    let timeout = Duration::from_secs(provider.request_timeout_seconds.get() as u64);
    let request = probe_request();
    let outcome = match kind {
        ProbeKind::Chat => match OpenAiChatClient::new(
            provider.base_url.as_str(),
            &api_key,
            model.remote_name.as_str(),
            timeout,
        ) {
            Ok(client) => client
                .complete(&request)
                .await
                .map(|response| (VerifiedCapability::Supported, response.latency_ms)),
            Err(error) => Err(error),
        },
        ProbeKind::Streaming => match OpenAiStreamClient::new(
            provider.base_url.as_str(),
            &api_key,
            model.remote_name.as_str(),
            timeout,
        ) {
            Ok(client) => client
                .complete(&request)
                .await
                .map(|response| (VerifiedCapability::Supported, response.latency_ms)),
            Err(error) => Err(error),
        },
        ProbeKind::Tools => match OpenAiToolsClient::new(
            provider.base_url.as_str(),
            &api_key,
            model.remote_name.as_str(),
            timeout,
        ) {
            Ok(client) => client.probe().await.map(|result| {
                (
                    match result {
                        ToolProbeResult::Supported(_) => VerifiedCapability::Supported,
                        ToolProbeResult::Unsupported => VerifiedCapability::Unsupported,
                    },
                    0,
                )
            }),
            Err(error) => Err(error),
        },
    };
    match outcome {
        Ok((verified, latency)) => ProbeResult {
            provider_id: model.provider_id.clone(),
            model_id: model.id.clone(),
            kind,
            status: ProbeStatus::Succeeded,
            verified,
            latency_ms: NonNegativeI64::new(
                "latency_ms",
                i64::try_from(latency).unwrap_or(i64::MAX),
            )
            .unwrap_or_else(|_| NonNegativeI64::new("latency_ms", 0).unwrap()),
            error_code: None,
        },
        Err(error) => failed_probe(provider, model, kind, error.kind(), 0),
    }
}

async fn provider_destination_allowed(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    if env::var("NOCTIS_PROVIDER_HOST_ALLOWLIST")
        .ok()
        .is_some_and(|value| {
            value
                .split(',')
                .map(str::trim)
                .any(|allowed| allowed == host)
        })
    {
        return true;
    }
    let port = url.port_or_known_default().unwrap_or(443);
    tokio::net::lookup_host((host, port))
        .await
        .is_ok_and(|addresses| addresses.map(|address| address.ip()).all(public_ip))
}

fn public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            !address.is_private()
                && !address.is_loopback()
                && !address.is_link_local()
                && !address.is_broadcast()
                && !address.is_documentation()
                && !address.is_multicast()
                && !address.is_unspecified()
        }
        IpAddr::V6(address) => {
            !address.is_loopback()
                && !address.is_unique_local()
                && !address.is_unicast_link_local()
                && !address.is_multicast()
                && !address.is_unspecified()
        }
    }
}

async fn method_not_allowed(Extension(request_id): Extension<RequestId>) -> AppError {
    AppError::method_not_allowed(request_id, json!({"method": "not allowed"}))
}

fn failed_probe(
    _provider: &Provider,
    model: &Model,
    kind: ProbeKind,
    error: ModelErrorKind,
    latency: i64,
) -> ProbeResult {
    ProbeResult {
        provider_id: model.provider_id.clone(),
        model_id: model.id.clone(),
        kind,
        status: ProbeStatus::Failed,
        verified: VerifiedCapability::Unsupported,
        latency_ms: NonNegativeI64::new("latency_ms", latency)
            .expect("probe latency is non-negative"),
        error_code: Some(match error {
            ModelErrorKind::AuthenticationFailed => ProbeErrorCode::AuthenticationFailed,
            ModelErrorKind::RateLimited => ProbeErrorCode::RateLimited,
            ModelErrorKind::Timeout => ProbeErrorCode::Timeout,
            ModelErrorKind::InvalidResponse => ProbeErrorCode::InvalidResponse,
            ModelErrorKind::ProviderUnavailable => ProbeErrorCode::ProviderUnavailable,
            ModelErrorKind::ContextTooLarge => ProbeErrorCode::ContextTooLarge,
            // Probe memanggil provider langsung, tanpa gerbang budget, sehingga kode ini tidak pernah muncul di sini.
            ModelErrorKind::BudgetExceeded => ProbeErrorCode::InvalidResponse,
        }),
    }
}

fn probe_request() -> ModelRequest {
    ModelRequest {
        project_id: "probe".into(),
        task_id: "probe".into(),
        agent_run_id: "probe".into(),
        model_class: "probe".into(),
        messages: vec![Message {
            role: MessageRole::User,
            content: "Reply with exactly: OK".into(),
            tool_call_id: None,
        }],
        tools: Vec::new(),
        limits: ModelLimits {
            max_input_tokens: 256,
            max_output_tokens: 32,
        },
    }
}

async fn mutate<F, Fut, T>(
    state: &ApiState,
    headers: &HeaderMap,
    method: Method,
    route: &str,
    body: &Value,
    request_id: RequestId,
    operation: F,
) -> Result<Response, AppError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(StatusCode, T), AppError>>,
    T: serde::Serialize,
{
    let key = idempotency_key(headers, request_id)?;
    let fingerprint = format!(
        "{method}:{route}:{}",
        serde_json::to_string(body).expect("JSON value serializes")
    );
    match state
        .idempotency
        .reserve(key, &fingerprint)
        .await
        .map_err(|error| store_error(error, request_id))?
    {
        Reservation::Conflict => {
            return Err(AppError::conflict(
                request_id,
                json!({"field": "Idempotency-Key", "reason": "reused for a different mutation"}),
            ));
        }
        Reservation::InProgress => {
            return Err(AppError::conflict(
                request_id,
                json!({"field": "Idempotency-Key", "reason": "mutation is in progress"}),
            ));
        }
        Reservation::Replay { status_code, body } => {
            let status = StatusCode::from_u16(status_code as u16)
                .map_err(|error| AppError::internal(request_id, error))?;
            return Ok((status, Json(body)).into_response());
        }
        Reservation::New => {}
    }
    match operation().await {
        Ok((status, value)) => {
            let body = serde_json::to_value(value)
                .map_err(|error| AppError::internal(request_id, error))?;
            if let Err(error) = state
                .idempotency
                .complete(key, &fingerprint, status.as_u16() as i16, &body)
                .await
            {
                let _ = state.idempotency.cancel(key, &fingerprint).await;
                return Err(store_error(error, request_id));
            }
            Ok((status, Json(body)).into_response())
        }
        Err(error) => {
            state
                .idempotency
                .cancel(key, &fingerprint)
                .await
                .map_err(|cancel| store_error(cancel, request_id))?;
            Err(error)
        }
    }
}

fn response<T: serde::Serialize>(
    status: StatusCode,
    value: T,
    request_id: RequestId,
) -> Result<(StatusCode, T), AppError> {
    serde_json::to_value(&value).map_err(|error| AppError::internal(request_id, error))?;
    Ok((status, value))
}

fn idempotency_key(headers: &HeaderMap, request_id: RequestId) -> Result<&str, AppError> {
    let key = headers
        .get(IDEMPOTENCY_KEY)
        .ok_or_else(|| {
            AppError::bad_request(
                request_id,
                json!({"field": "Idempotency-Key", "reason": "required"}),
            )
        })?
        .to_str()
        .map_err(|_| {
            AppError::bad_request(
                request_id,
                json!({"field": "Idempotency-Key", "reason": "must be valid ASCII"}),
            )
        })?;
    if key.trim().is_empty() || key.len() > 255 {
        return Err(AppError::bad_request(
            request_id,
            json!({"field": "Idempotency-Key", "reason": "must contain 1 to 255 bytes"}),
        ));
    }
    Ok(key)
}

fn body(
    payload: Result<Json<Value>, JsonRejection>,
    request_id: RequestId,
) -> Result<Value, AppError> {
    payload
        .map(|Json(value)| value)
        .map_err(|_| AppError::bad_request(request_id, json!({"body": "invalid JSON"})))
}

fn parse<T: serde::de::DeserializeOwned>(
    body: &Value,
    request_id: RequestId,
) -> Result<T, AppError> {
    serde_json::from_value(body.clone()).map_err(|_| {
        AppError::unprocessable(request_id, json!({"body": "does not match contract"}))
    })
}

fn validation(field: &'static str, message: &'static str, request_id: RequestId) -> AppError {
    AppError::unprocessable(request_id, json!({"field": field, "reason": message}))
}

fn store_error(error: StoreError, request_id: RequestId) -> AppError {
    match error {
        StoreError::NotFound => AppError::not_found(request_id, Value::Null),
        StoreError::Conflict(conflict) => AppError::conflict(
            request_id,
            json!({"resource": match conflict { Conflict::ProviderId => "provider_id", Conflict::ModelId => "model_id", Conflict::RemoteName => "remote_name" }}),
        ),
        StoreError::InvalidRow(error) => validation(error.field, error.message, request_id),
        error => AppError::internal(request_id, error),
    }
}

fn kind_name(kind: ProbeKind) -> &'static str {
    match kind {
        ProbeKind::Chat => "chat",
        ProbeKind::Streaming => "streaming",
        ProbeKind::Tools => "tools",
    }
}

#[cfg(test)]
mod tests {
    use super::public_ip;

    #[test]
    fn blocks_non_public_provider_addresses() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.0.1",
            "169.254.169.254",
            "::1",
            "fc00::1",
            "fe80::1",
        ] {
            assert!(!public_ip(address.parse().unwrap()), "accepted {address}");
        }
        assert!(public_ip("1.1.1.1".parse().unwrap()));
        assert!(public_ip("2606:4700:4700::1111".parse().unwrap()));
    }
}
