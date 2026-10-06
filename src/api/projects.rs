use std::{future::Future, path::Path};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Extension, Path as RoutePath, State, rejection::JsonRejection},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get as route_get, post},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::{AppError, RequestId};
use crate::{
    api::contracts::project::{ProjectInput, ProjectRunInput},
    context::discovery::discover,
    domain::project::{Project, ProjectRun, RepositoryPath},
    store::{
        idempotency::{IdempotencyRepository, Reservation},
        project::{ProjectRepository, ProjectStoreError},
    },
};

#[derive(Clone)]
pub(crate) struct StateData {
    pub store: ProjectRepository,
    pub idempotency: IdempotencyRepository,
    pub pool: PgPool,
    /// Model untuk Lead Agent bila request tidak menyebut `model_id`.
    pub default_model: Option<String>,
    /// Jumlah slot worker yang dikonfigurasi (`scheduler.max_parallel_agents`), untuk tampilan dashboard.
    pub max_slots: usize,
}

const DEFAULT_MAX_SLOTS: usize = 2;

pub fn router(pool: PgPool) -> Router {
    router_with_default_model(pool, None)
}

pub fn router_with_default_model(pool: PgPool, default_model: Option<String>) -> Router {
    router_with_scheduler(pool, default_model, DEFAULT_MAX_SLOTS)
}

pub fn router_with_scheduler(
    pool: PgPool,
    default_model: Option<String>,
    max_slots: usize,
) -> Router {
    let state = StateData {
        max_slots,
        store: ProjectRepository::new(pool.clone()),
        idempotency: IdempotencyRepository::new(pool.clone()),
        pool,
        default_model,
    };
    Router::new()
        .route("/api/v1/projects", route_get(list).post(create))
        .route(
            "/api/v1/projects/{id}",
            route_get(get).put(update).delete(delete),
        )
        .route("/api/v1/approvals", route_get(approvals))
        .route("/api/v1/projects/{id}/discover", post(discovery))
        .route(
            "/api/v1/projects/{id}/runs",
            route_get(list_runs).post(create_run),
        )
        .merge(super::runs::routes())
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(state)
}

pub(crate) fn body(
    payload: Result<Json<Value>, JsonRejection>,
    id: RequestId,
) -> Result<Value, AppError> {
    payload
        .map(|Json(value)| value)
        .map_err(|_| AppError::bad_request(id, json!({"body":"invalid JSON"})))
}

pub(crate) fn parse<T: DeserializeOwned>(value: &Value, id: RequestId) -> Result<T, AppError> {
    serde_json::from_value(value.clone()).map_err(|_| invalid(id))
}

pub(crate) fn invalid(id: RequestId) -> AppError {
    AppError::unprocessable(id, json!({"body":"does not match contract"}))
}

pub(crate) fn store_error(error: ProjectStoreError, id: RequestId) -> AppError {
    match error {
        ProjectStoreError::NotFound => AppError::not_found(id, Value::Null),
        ProjectStoreError::Conflict => AppError::conflict(id, Value::Null),
        ProjectStoreError::Invalid(_) | ProjectStoreError::InvalidId(_) => invalid(id),
        ProjectStoreError::Database(error) => AppError::internal(id, error),
        ProjectStoreError::Json(error) => AppError::internal(id, error),
    }
}

pub(crate) async fn mutate<T, F, Fut>(
    state: &StateData,
    headers: &HeaderMap,
    method: Method,
    route: &str,
    body: &Value,
    id: RequestId,
    operation: F,
) -> Result<Response, AppError>
where
    T: Serialize,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(StatusCode, T), AppError>>,
{
    let key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| {
            !value.is_empty() && value.len() <= 255 && value.bytes().all(|b| b.is_ascii_graphic())
        })
        .ok_or_else(|| {
            AppError::bad_request(
                id,
                json!({"field":"Idempotency-Key","reason":"required, 1 to 255 ASCII bytes"}),
            )
        })?;
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(method.as_str(), route, body))
                .map_err(|error| AppError::internal(id, error))?
        )
    );
    match state
        .idempotency
        .reserve(key, &fingerprint)
        .await
        .map_err(|error| AppError::internal(id, error))?
    {
        Reservation::Conflict | Reservation::InProgress => {
            return Err(AppError::conflict(id, json!({"field":"Idempotency-Key"})));
        }
        Reservation::Replay { status_code, body } => {
            return Ok((
                StatusCode::from_u16(status_code as u16)
                    .map_err(|error| AppError::internal(id, error))?,
                Json(body),
            )
                .into_response());
        }
        Reservation::New => {}
    }
    let result = operation().await;
    match result {
        Ok((status, value)) => {
            let value =
                serde_json::to_value(value).map_err(|error| AppError::internal(id, error))?;
            if let Err(error) = state
                .idempotency
                .complete(key, &fingerprint, status.as_u16() as i16, &value)
                .await
            {
                // ponytail: store mutation and replay record are separate; retry after a failed completion may conflict.
                return Err(AppError::internal(id, error));
            }
            Ok((status, Json(value)).into_response())
        }
        Err(error) => {
            if let Err(cancel_error) = state.idempotency.cancel(key, &fingerprint).await {
                return Err(AppError::internal(id, cancel_error));
            }
            Err(error)
        }
    }
}

async fn list(
    State(state): State<StateData>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<Value>, AppError> {
    let rows = sqlx::query("SELECT id FROM projects ORDER BY created_at,id LIMIT 100")
        .fetch_all(&state.pool)
        .await
        .map_err(|error| AppError::internal(id, error))?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let key: Uuid = row.get("id");
        items.push(
            state
                .store
                .get_project(&key.to_string())
                .await
                .map_err(|error| store_error(error, id))?,
        );
    }
    Ok(Json(json!({"items":items})))
}

/// Antrean keputusan manusia: plan menunggu, task NEEDS_HUMAN/CONFLICT, dan riwayat keputusan (hanya baca).
async fn approvals(
    State(state): State<StateData>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<Value>, AppError> {
    let snapshot = crate::store::approvals::approvals_snapshot(&state.pool)
        .await
        .map_err(|error| AppError::internal(id, error))?;
    Ok(Json(json!(snapshot)))
}

async fn get(
    State(state): State<StateData>,
    Extension(id): Extension<RequestId>,
    RoutePath(project_id): RoutePath<String>,
) -> Result<Json<Value>, AppError> {
    let project = state
        .store
        .get_project(&project_id)
        .await
        .map_err(|error| store_error(error, id))?;
    Ok(Json(json!({"project":project})))
}

async fn create(
    State(state): State<StateData>,
    Extension(id): Extension<RequestId>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    let value = body(payload, id)?;
    mutate(
        &state,
        &headers,
        Method::POST,
        "/api/v1/projects",
        &value,
        id,
        || async {
            let project =
                Project::try_from(parse::<ProjectInput>(&value, id)?).map_err(|_| invalid(id))?;
            state
                .store
                .create_project(&project)
                .await
                .map_err(|error| store_error(error, id))?;
            Ok((StatusCode::CREATED, json!({"project":project})))
        },
    )
    .await
}

// ponytail: update/delete compare existing values because projects table has no version column; add versioned store operation when schema permits.
async fn update(
    State(state): State<StateData>,
    Extension(id): Extension<RequestId>,
    RoutePath(project_id): RoutePath<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    let value = body(payload, id)?;
    let route = format!("/api/v1/projects/{project_id}");
    mutate(&state, &headers, Method::PUT, &route, &value, id, || async {
        let input: UpdateInput = parse(&value, id)?;
        let project = Project::try_from(input.project).map_err(|_| invalid(id))?;
        if project.id.as_str() != project_id { return Err(AppError::conflict(id, Value::Null)); }
        let old = RepositoryPath::parse(input.expected_repository_path).map_err(|_| invalid(id))?;
        let key = canonical_uuid(&project_id, id)?;
        let result = sqlx::query("UPDATE projects SET name=$1,repository_path=$2,updated_at=now() WHERE id=$3 AND name=$4 AND repository_path=$5 AND NOT EXISTS (SELECT 1 FROM project_runs WHERE project_id=$3)")
            .bind(project.name.as_str()).bind(project.repository_path.as_path().to_str())
            .bind(key).bind(input.expected_name).bind(old.as_path().to_str())
            .execute(&state.pool).await.map_err(|error| db_conflict(error, id))?;
        if result.rows_affected() == 0 { return Err(AppError::conflict(id, Value::Null)); }
        Ok((StatusCode::OK, json!({"project":project})))
    }).await
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateInput {
    expected_name: String,
    expected_repository_path: String,
    #[serde(flatten)]
    project: ProjectInput,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteInput {
    expected_name: String,
    expected_repository_path: String,
}

async fn delete(
    State(state): State<StateData>,
    Extension(id): Extension<RequestId>,
    RoutePath(project_id): RoutePath<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    let value = body(payload, id)?;
    let route = format!("/api/v1/projects/{project_id}");
    mutate(&state, &headers, Method::DELETE, &route, &value, id, || async {
        let input: DeleteInput = parse(&value, id)?;
        let old = RepositoryPath::parse(input.expected_repository_path).map_err(|_| invalid(id))?;
        let key = canonical_uuid(&project_id, id)?;
        let result = sqlx::query("DELETE FROM projects WHERE id=$1 AND name=$2 AND repository_path=$3 AND NOT EXISTS (SELECT 1 FROM project_runs WHERE project_id=$1)")
            .bind(key).bind(input.expected_name).bind(old.as_path().to_str())
            .execute(&state.pool).await.map_err(|error| db_conflict(error, id))?;
        if result.rows_affected() == 0 { return Err(AppError::conflict(id, Value::Null)); }
        Ok((StatusCode::OK, json!({"deleted":true,"id":project_id})))
    }).await
}

pub(crate) fn canonical_uuid(value: &str, id: RequestId) -> Result<Uuid, AppError> {
    let uuid = Uuid::parse_str(value).map_err(|_| invalid(id))?;
    if uuid.to_string() != value {
        return Err(invalid(id));
    }
    Ok(uuid)
}

pub(crate) fn db_conflict(error: sqlx::Error, id: RequestId) -> AppError {
    if error
        .as_database_error()
        .is_some_and(|db| db.is_unique_violation() || db.is_foreign_key_violation())
    {
        AppError::conflict(id, Value::Null)
    } else {
        AppError::internal(id, error)
    }
}

async fn discovery(
    State(state): State<StateData>,
    Extension(id): Extension<RequestId>,
    RoutePath(project_id): RoutePath<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    let value = body(payload, id)?;
    let route = format!("/api/v1/projects/{project_id}/discover");
    mutate(
        &state,
        &headers,
        Method::POST,
        &route,
        &value,
        id,
        || async {
            if value != json!({}) {
                return Err(invalid(id));
            }
            let project = state
                .store
                .get_project(&project_id)
                .await
                .map_err(|error| store_error(error, id))?;
            let path = project.repository_path.as_path();
            let valid = RepositoryPath::parse(path.to_str().unwrap_or(""))
                .map_err(|_| AppError::conflict(id, Value::Null))?;
            if valid.as_path() != Path::new(path) {
                return Err(AppError::conflict(id, Value::Null));
            }
            let map = tokio::task::spawn_blocking(move || discover(valid.as_path()))
                .await
                .map_err(|error| AppError::internal(id, error))?
                .map_err(|_| {
                    AppError::unprocessable(id, json!({"repository":"discovery failed"}))
                })?;
            Ok((StatusCode::OK, json!({"repository_map":map})))
        },
    )
    .await
}

async fn list_runs(
    State(state): State<StateData>,
    Extension(id): Extension<RequestId>,
    RoutePath(project_id): RoutePath<String>,
) -> Result<Json<Value>, AppError> {
    // get_project dulu supaya project yang tidak ada menjadi 404, bukan list kosong.
    state
        .store
        .get_project(&project_id)
        .await
        .map_err(|error| store_error(error, id))?;
    let runs = state
        .store
        .list_runs(&project_id)
        .await
        .map_err(|error| store_error(error, id))?;
    Ok(Json(json!({"items":runs})))
}

async fn create_run(
    State(state): State<StateData>,
    Extension(id): Extension<RequestId>,
    RoutePath(project_id): RoutePath<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    let value = body(payload, id)?;
    let route = format!("/api/v1/projects/{project_id}/runs");
    mutate(
        &state,
        &headers,
        Method::POST,
        &route,
        &value,
        id,
        || async {
            let run = ProjectRun::try_from(parse::<ProjectRunInput>(&value, id)?)
                .map_err(|_| invalid(id))?;
            if run.project_id.as_str() != project_id {
                return Err(AppError::conflict(id, Value::Null));
            }
            state
                .store
                .get_project(&project_id)
                .await
                .map_err(|error| store_error(error, id))?;
            state
                .store
                .create_run(&run)
                .await
                .map_err(|error| store_error(error, id))?;
            Ok((StatusCode::CREATED, json!({"run":run})))
        },
    )
    .await
}
