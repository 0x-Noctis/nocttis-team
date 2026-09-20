use axum::{
    Json, Router,
    body::Body,
    extract::{Extension, Path, State},
    http::{
        HeaderMap, StatusCode,
        header::{CACHE_CONTROL, CONTENT_TYPE},
    },
    response::Response,
    routing::get,
};
use serde_json::{Value, json};

use super::{
    AppError, RequestId,
    tasks::{TaskApiState, store_error},
};

pub(crate) fn router() -> Router<TaskApiState> {
    Router::new()
        .route("/api/v1/tasks/{id}/events", get(list_events))
        .route("/api/v1/tasks/{id}/events/stream", get(stream_events))
}

async fn list_events(
    State(state): State<TaskApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    state
        .tasks
        .get(&id)
        .await
        .map_err(|error| store_error(error, request_id))?;
    let events = state
        .tasks
        .events(&id)
        .await
        .map_err(|error| store_error(error, request_id))?;
    Ok(Json(
        json!({"items": events.into_iter().map(event_json).collect::<Vec<_>>(), "next_cursor": null}),
    ))
}

async fn stream_events(
    State(state): State<TaskApiState>,
    Extension(request_id): Extension<RequestId>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    state
        .tasks
        .get(&id)
        .await
        .map_err(|error| store_error(error, request_id))?;
    let cursor = match headers.get("last-event-id") {
        Some(value) => value
            .to_str()
            .ok()
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|value| *value >= 0)
            .ok_or_else(|| {
                AppError::bad_request(
                    request_id,
                    json!({"header":"Last-Event-ID","reason":"must be a non-negative integer"}),
                )
            })?,
        None => 0,
    };
    let mut body = String::new();
    for event in state
        .tasks
        .events(&id)
        .await
        .map_err(|error| store_error(error, request_id))?
        .into_iter()
        .filter(|event| event.id > cursor)
    {
        let id = event.id;
        let data = serde_json::to_string(&event_json(event))
            .map_err(|error| AppError::internal(request_id, error))?;
        body.push_str(&format!("id: {id}\nevent: task_event\ndata: {data}\n\n"));
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .header(CACHE_CONTROL, "no-cache")
        .body(Body::from(body))
        .map_err(|error| AppError::internal(request_id, error))
}

fn event_json(event: crate::store::event::TaskEvent) -> Value {
    json!({
        "id": event.id,
        "task_id": event.task_id,
        "actor": event.actor,
        "event_type": event.event_type,
        "from_status": event.from_status,
        "to_status": event.to_status,
        "payload": event.payload,
    })
}
