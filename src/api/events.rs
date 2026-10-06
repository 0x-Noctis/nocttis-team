use axum::{
    Json, Router,
    extract::{Extension, Path, State},
    http::HeaderMap,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use serde_json::{Value, json};
use std::{convert::Infallible, time::Duration};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

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
    let (sender, receiver) = mpsc::channel::<Result<Event, Infallible>>(16);
    tokio::spawn(async move {
        let mut cursor = cursor;
        loop {
            let events = match state.tasks.events_after(&id, cursor).await {
                Ok(events) => events,
                Err(error) => {
                    tracing::error!(task_id = %id, error = %error, "event stream polling failed");
                    return;
                }
            };
            for event in events {
                let event_id = event.id;
                let message = match Event::default()
                    .id(event_id.to_string())
                    .event("task_event")
                    .json_data(event_json(event))
                {
                    Ok(message) => message,
                    Err(_) => return,
                };
                if sender.send(Ok(message)).await.is_err() {
                    return;
                }
                cursor = event_id;
            }
            tokio::select! {
                _ = sender.closed() => return,
                _ = tokio::time::sleep(Duration::from_millis(200)) => {}
            }
        }
    });
    Ok(Sse::new(ReceiverStream::new(receiver))
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(5))
                .text("keep-alive"),
        )
        .into_response())
}

fn event_json(event: crate::store::event::TaskEvent) -> Value {
    json!({
        "id": event.id,
        "task_id": event.task_id,
        "actor": event.actor,
        "actor_id": event.actor_id,
        "event_type": event.event_type,
        "from_status": event.from_status,
        "to_status": event.to_status,
        "payload": event.payload,
    })
}
