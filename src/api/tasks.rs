use std::{future::Future, sync::Arc};

use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    extract::{
        Extension, Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, Method, StatusCode, header::HeaderName},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::{
    api::contracts::task::TaskContractInput,
    domain::{
        state_machine::Actor,
        task::{TaskContract, TaskStatus},
    },
    store::{
        artifact::ArtifactStore,
        idempotency::{IdempotencyRepository, Reservation},
        task::{Conflict, StoreError, StoredTask, TaskRepository},
    },
};

use super::{AppError, RequestId};

const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");

#[derive(Clone)]
pub(crate) struct TaskApiState {
    pub tasks: TaskRepository,
    pub idempotency: IdempotencyRepository,
    pub pool: PgPool,
    pub artifacts: Arc<ArtifactStore>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pagination {
    cursor: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionInput {
    expected_version: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateInput {
    expected_version: i64,
    #[serde(flatten)]
    contract: TaskContractInput,
}

#[derive(Serialize)]
pub(crate) struct TaskResponse {
    contract: TaskContract,
    status: TaskStatus,
    version: i64,
}

impl From<StoredTask> for TaskResponse {
    fn from(task: StoredTask) -> Self {
        Self {
            contract: task.contract,
            status: task.status,
            version: task.version,
        }
    }
}

fn default_limit() -> i64 {
    50
}

pub fn router(pool: PgPool, artifacts: ArtifactStore) -> Router {
    let state = TaskApiState {
        tasks: TaskRepository::new(pool.clone()),
        idempotency: IdempotencyRepository::new(pool.clone()),
        pool,
        artifacts: Arc::new(artifacts),
    };
    Router::new()
        .route("/api/v1/tasks", get(list_tasks).post(create_task))
        .route(
            "/api/v1/tasks/{id}",
            get(get_task).put(update_task).delete(delete_task),
        )
        .route("/api/v1/tasks/{id}/start", post(start_task))
        .route("/api/v1/tasks/{id}/cancel", post(cancel_task))
        .route("/api/v1/tasks/{id}/retry", post(retry_task))
        .merge(super::events::router())
        .merge(super::artifacts::router())
        .layer(DefaultBodyLimit::max(64 * 1024))
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(state)
}

async fn list_tasks(
    State(state): State<TaskApiState>,
    Extension(request_id): Extension<RequestId>,
    page: Result<Query<Pagination>, QueryRejection>,
) -> Result<Json<Value>, AppError> {
    let Query(page) =
        page.map_err(|_| AppError::bad_request(request_id, json!({"query":"invalid pagination"})))?;
    let page = state
        .tasks
        .list(page.cursor.as_deref(), page.limit)
        .await
        .map_err(|error| store_error(error, request_id))?;
    Ok(Json(
        json!({"items": page.items.into_iter().map(TaskResponse::from).collect::<Vec<_>>(), "next_cursor": page.next_cursor}),
    ))
}

async fn get_task(
    State(state): State<TaskApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(id): Path<String>,
) -> Result<Json<TaskResponse>, AppError> {
    Ok(Json(
        state
            .tasks
            .get(&id)
            .await
            .map(TaskResponse::from)
            .map_err(|error| store_error(error, request_id))?,
    ))
}

async fn create_task(
    State(state): State<TaskApiState>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    let body = body(payload, request_id)?;
    mutate(
        &state,
        &headers,
        Method::POST,
        "/api/v1/tasks",
        &body,
        request_id,
        || async {
            let contract = TaskContract::try_from(parse::<TaskContractInput>(&body, request_id)?)
                .map_err(|_| validation(request_id))?;
            let task = state
                .tasks
                .create(&contract)
                .await
                .map_err(|error| store_error(error, request_id))?;
            Ok((StatusCode::CREATED, TaskResponse::from(task)))
        },
    )
    .await
}

async fn update_task(
    State(state): State<TaskApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    let body = body(payload, request_id)?;
    let route = format!("/api/v1/tasks/{id}");
    mutate(
        &state,
        &headers,
        Method::PUT,
        &route,
        &body,
        request_id,
        || async {
            let input: UpdateInput = parse(&body, request_id)?;
            let contract =
                TaskContract::try_from(input.contract).map_err(|_| validation(request_id))?;
            if contract.id.as_str() != id {
                return Err(AppError::unprocessable(
                    request_id,
                    json!({"field":"id","reason":"must match route"}),
                ));
            }
            let task = state
                .tasks
                .update(&contract, input.expected_version)
                .await
                .map_err(|error| store_error(error, request_id))?;
            Ok((StatusCode::OK, TaskResponse::from(task)))
        },
    )
    .await
}

async fn delete_task(
    State(state): State<TaskApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    let body = body(payload, request_id)?;
    let route = format!("/api/v1/tasks/{id}");
    mutate(
        &state,
        &headers,
        Method::DELETE,
        &route,
        &body,
        request_id,
        || async {
            let input: VersionInput = parse(&body, request_id)?;
            state
                .tasks
                .delete(&id, input.expected_version)
                .await
                .map_err(|error| store_error(error, request_id))?;
            Ok((StatusCode::OK, json!({"deleted":true,"id":id})))
        },
    )
    .await
}

async fn start_task(
    state: State<TaskApiState>,
    request_id: Extension<RequestId>,
    path: Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    transition_task(
        state,
        request_id,
        path,
        headers,
        payload,
        "start",
        TaskStatus::Running,
    )
    .await
}
async fn cancel_task(
    state: State<TaskApiState>,
    request_id: Extension<RequestId>,
    path: Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    transition_task(
        state,
        request_id,
        path,
        headers,
        payload,
        "cancel",
        TaskStatus::Cancelled,
    )
    .await
}
async fn retry_task(
    state: State<TaskApiState>,
    request_id: Extension<RequestId>,
    path: Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    transition_task(
        state,
        request_id,
        path,
        headers,
        payload,
        "retry",
        TaskStatus::Ready,
    )
    .await
}

async fn transition_task(
    State(state): State<TaskApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
    action: &'static str,
    status: TaskStatus,
) -> Result<Response, AppError> {
    let body = body(payload, request_id)?;
    let route = format!("/api/v1/tasks/{id}/{action}");
    mutate(
        &state,
        &headers,
        Method::POST,
        &route,
        &body,
        request_id,
        || async {
            let input: VersionInput = parse(&body, request_id)?;
            let task = state
                .tasks
                .transition(&id, input.expected_version, status, Actor::System)
                .await
                .map_err(|error| store_error(error, request_id))?;
            Ok((StatusCode::OK, TaskResponse::from(task)))
        },
    )
    .await
}

async fn mutate<T, F, Fut>(
    state: &TaskApiState,
    headers: &HeaderMap,
    method: Method,
    route: &str,
    body: &Value,
    request_id: RequestId,
    operation: F,
) -> Result<Response, AppError>
where
    T: Serialize,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(StatusCode, T), AppError>>,
{
    let key = idempotency_key(headers, request_id)?;
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(method.as_str(), route, body))
                .map_err(|error| AppError::internal(request_id, error))?
        )
    );
    match state
        .idempotency
        .reserve(key, &fingerprint)
        .await
        .map_err(|error| AppError::internal(request_id, error))?
    {
        Reservation::Conflict => {
            return Err(AppError::conflict(
                request_id,
                json!({"field":"Idempotency-Key","reason":"reused for a different mutation"}),
            ));
        }
        Reservation::InProgress => {
            return Err(AppError::conflict(
                request_id,
                json!({"field":"Idempotency-Key","reason":"mutation is in progress"}),
            ));
        }
        Reservation::Replay { status_code, body } => {
            return Ok((
                StatusCode::from_u16(status_code as u16)
                    .map_err(|error| AppError::internal(request_id, error))?,
                Json(body),
            )
                .into_response());
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
                return Err(AppError::internal(request_id, error));
            }
            Ok((status, Json(body)).into_response())
        }
        Err(error) => {
            let _ = state.idempotency.cancel(key, &fingerprint).await;
            Err(error)
        }
    }
}

fn idempotency_key(headers: &HeaderMap, request_id: RequestId) -> Result<&str, AppError> {
    let key = headers
        .get(IDEMPOTENCY_KEY)
        .ok_or_else(|| {
            AppError::bad_request(
                request_id,
                json!({"field":"Idempotency-Key","reason":"required"}),
            )
        })?
        .to_str()
        .map_err(|_| {
            AppError::bad_request(
                request_id,
                json!({"field":"Idempotency-Key","reason":"must be valid ASCII"}),
            )
        })?;
    if key.trim().is_empty() || key.len() > 255 {
        return Err(AppError::bad_request(
            request_id,
            json!({"field":"Idempotency-Key","reason":"must contain 1 to 255 bytes"}),
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
        .map_err(|_| AppError::bad_request(request_id, json!({"body":"invalid JSON"})))
}
fn parse<T: DeserializeOwned>(body: &Value, request_id: RequestId) -> Result<T, AppError> {
    serde_json::from_value(body.clone()).map_err(|_| validation(request_id))
}
fn validation(request_id: RequestId) -> AppError {
    AppError::unprocessable(request_id, json!({"body":"does not match contract"}))
}

pub(crate) fn store_error(error: StoreError, request_id: RequestId) -> AppError {
    match error {
        StoreError::NotFound => AppError::not_found(request_id, json!({"resource":"task"})),
        StoreError::Conflict(Conflict::TaskId) => {
            AppError::conflict(request_id, json!({"field":"id"}))
        }
        StoreError::Conflict(_) | StoreError::InvalidTransition(_) => AppError::conflict(
            request_id,
            json!({"reason":"version or transition conflict"}),
        ),
        StoreError::InvalidRow(_)
        | StoreError::InvalidId(_)
        | StoreError::InvalidNumeric(_)
        | StoreError::InvalidJson(_) => validation(request_id),
        StoreError::Database(error) => AppError::internal(request_id, error),
    }
}

async fn method_not_allowed(Extension(request_id): Extension<RequestId>) -> AppError {
    AppError::method_not_allowed(request_id, json!({}))
}
