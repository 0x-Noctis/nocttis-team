use axum::{
    Json, Router,
    body::Body,
    extract::{Extension, Path, State},
    http::{
        StatusCode,
        header::{CONTENT_DISPOSITION, CONTENT_TYPE},
    },
    response::Response,
    routing::get,
};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use super::{
    AppError, RequestId,
    tasks::{TaskApiState, store_error},
};
use crate::store::artifact::ArtifactError;

#[derive(Serialize)]
struct ArtifactResponse {
    id: Uuid,
    kind: String,
    size_bytes: i64,
    sha256: String,
}

pub(crate) fn router() -> Router<TaskApiState> {
    Router::new()
        .route("/api/v1/tasks/{id}/artifacts", get(list_artifacts))
        .route(
            "/api/v1/tasks/{id}/artifacts/{artifact_id}",
            get(download_artifact),
        )
        .route("/api/v1/tasks/{id}/diff", get(download_diff))
}

async fn list_artifacts(
    State(state): State<TaskApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    state
        .tasks
        .get(&id)
        .await
        .map_err(|error| store_error(error, request_id))?;
    let rows = sqlx::query(
        "SELECT id,kind,size_bytes,sha256 FROM artifacts WHERE task_id=$1 ORDER BY created_at,id",
    )
    .bind(&id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::internal(request_id, error))?;
    let items = rows
        .into_iter()
        .map(|row| ArtifactResponse {
            id: row.get("id"),
            kind: row.get("kind"),
            size_bytes: row.get("size_bytes"),
            sha256: row.get("sha256"),
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({"items":items,"next_cursor":null})))
}

async fn download_artifact(
    State(state): State<TaskApiState>,
    Extension(request_id): Extension<RequestId>,
    Path((id, artifact_id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let artifact_id = canonical_uuid(&artifact_id, request_id)?;
    let row = sqlx::query("SELECT kind,size_bytes FROM artifacts WHERE task_id=$1 AND id=$2")
        .bind(&id)
        .bind(artifact_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|error| AppError::internal(request_id, error))?
        .ok_or_else(|| AppError::not_found(request_id, json!({"resource":"artifact"})))?;
    artifact_response(
        &state,
        artifact_id,
        row.get("kind"),
        row.get("size_bytes"),
        request_id,
    )
}

async fn download_diff(
    State(state): State<TaskApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    state
        .tasks
        .get(&id)
        .await
        .map_err(|error| store_error(error, request_id))?;
    let row = sqlx::query("SELECT id,kind,size_bytes FROM artifacts WHERE task_id=$1 AND kind IN ('diff','patch') ORDER BY created_at DESC,id DESC LIMIT 1")
        .bind(&id).fetch_optional(&state.pool).await.map_err(|error| AppError::internal(request_id,error))?
        .ok_or_else(|| AppError::not_found(request_id,json!({"resource":"diff"})))?;
    artifact_response(
        &state,
        row.get("id"),
        row.get("kind"),
        row.get("size_bytes"),
        request_id,
    )
}

fn artifact_response(
    state: &TaskApiState,
    artifact_id: Uuid,
    kind: String,
    size: i64,
    request_id: RequestId,
) -> Result<Response, AppError> {
    let maximum = u64::try_from(size)
        .map_err(|_| AppError::internal(request_id, anyhow::anyhow!("invalid artifact size")))?;
    let bytes = state
        .artifacts
        .read_bounded(&artifact_id.to_string(), maximum)
        .map_err(|error| artifact_error(error, request_id))?;
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/octet-stream")
        .header(
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"{artifact_id}.{kind}\""),
        )
        .body(Body::from(bytes))
        .map_err(|error| AppError::internal(request_id, error))
}

fn canonical_uuid(value: &str, request_id: RequestId) -> Result<Uuid, AppError> {
    let parsed = Uuid::parse_str(value)
        .map_err(|_| AppError::not_found(request_id, json!({"resource":"artifact"})))?;
    if parsed.to_string() != value {
        return Err(AppError::not_found(
            request_id,
            json!({"resource":"artifact"}),
        ));
    }
    Ok(parsed)
}
fn artifact_error(error: ArtifactError, request_id: RequestId) -> AppError {
    match error {
        ArtifactError::NotFound | ArtifactError::InvalidArtifactId => {
            AppError::not_found(request_id, json!({"resource":"artifact"}))
        }
        ArtifactError::TooLarge => AppError::unprocessable(
            request_id,
            json!({"resource":"artifact","reason":"too large"}),
        ),
        _ => AppError::internal(request_id, error),
    }
}
