use axum::{
    Json, Router,
    extract::{Extension, Path, State, rejection::JsonRejection},
    http::{HeaderMap, Method, StatusCode},
    response::Response,
    routing::{get as route_get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    AppError, RequestId,
    projects::{StateData, body, canonical_uuid, invalid, mutate, parse, store_error},
};
use crate::{
    api::contracts::plan::{PlanApprovalInput, ProposedPlanInput},
    domain::project::{ApprovalDecision, PlanApproval, ProposedPlan, RunStatus},
};

pub(crate) fn routes() -> Router<StateData> {
    Router::new()
        .route("/api/v1/runs/{id}", route_get(get))
        .route("/api/v1/runs/{id}/plan", post(propose))
        .route("/api/v1/runs/{id}/approve-plan", post(approve))
        .route("/api/v1/runs/{id}/reject-plan", post(reject))
        .route("/api/v1/runs/{id}/pause", post(pause))
        .route("/api/v1/runs/{id}/resume", post(resume))
        .route("/api/v1/runs/{id}/cancel", post(cancel))
}

async fn get(
    State(state): State<StateData>,
    Extension(request_id): Extension<RequestId>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let run = state
        .store
        .get_run(&id)
        .await
        .map_err(|error| store_error(error, request_id))?;
    Ok(Json(json!({"run":run})))
}

async fn propose(
    State(state): State<StateData>,
    Extension(request_id): Extension<RequestId>,
    Path(id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    let value = body(payload, request_id)?;
    let route = format!("/api/v1/runs/{id}/plan");
    mutate(
        &state,
        &headers,
        Method::POST,
        &route,
        &value,
        request_id,
        || async {
            let plan = ProposedPlan::try_from(parse::<ProposedPlanInput>(&value, request_id)?)
                .map_err(|_| invalid(request_id))?;
            if plan.project_run_id.as_str() != id {
                return Err(AppError::conflict(request_id, Value::Null));
            }
            state
                .store
                .propose(&plan)
                .await
                .map_err(|error| store_error(error, request_id))?;
            Ok((StatusCode::CREATED, json!({"plan":plan})))
        },
    )
    .await
}

async fn approve(
    state: State<StateData>,
    request_id: Extension<RequestId>,
    path: Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    decide(
        state,
        request_id,
        path,
        headers,
        payload,
        ApprovalDecision::Approved,
    )
    .await
}
async fn reject(
    state: State<StateData>,
    request_id: Extension<RequestId>,
    path: Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    decide(
        state,
        request_id,
        path,
        headers,
        payload,
        ApprovalDecision::Rejected,
    )
    .await
}

async fn decide(
    State(state): State<StateData>,
    Extension(request_id): Extension<RequestId>,
    Path(id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
    decision: ApprovalDecision,
) -> Result<Response, AppError> {
    let value = body(payload, request_id)?;
    let action = if decision == ApprovalDecision::Approved {
        "approve-plan"
    } else {
        "reject-plan"
    };
    let route = format!("/api/v1/runs/{id}/{action}");
    mutate(&state, &headers, Method::POST, &route, &value, request_id, || async {
        let approval = PlanApproval::try_from(parse::<PlanApprovalInput>(&value, request_id)?).map_err(|_| invalid(request_id))?;
        if approval.decision != decision { return Err(invalid(request_id)); }
        let plan = state.store.get_plan(approval.plan_id.as_str()).await.map_err(|error| store_error(error, request_id))?;
        if plan.project_run_id.as_str() != id { return Err(AppError::conflict(request_id, Value::Null)); }
        state.store.decide(&approval).await.map_err(|error| store_error(error, request_id))?;
        Ok((StatusCode::OK, json!({"plan":state.store.get_plan(approval.plan_id.as_str()).await.map_err(|error| store_error(error, request_id))?})))
    }).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Transition {
    expected_status: RunStatus,
}

async fn pause(
    state: State<StateData>,
    request_id: Extension<RequestId>,
    path: Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    transition(
        state,
        request_id,
        path,
        headers,
        payload,
        "pause",
        ("RUNNING", "PAUSED"),
    )
    .await
}
async fn resume(
    state: State<StateData>,
    request_id: Extension<RequestId>,
    path: Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    transition(
        state,
        request_id,
        path,
        headers,
        payload,
        "resume",
        ("PAUSED", "RUNNING"),
    )
    .await
}
async fn cancel(
    state: State<StateData>,
    request_id: Extension<RequestId>,
    path: Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    transition(
        state,
        request_id,
        path,
        headers,
        payload,
        "cancel",
        ("RUNNING", "CANCELLED"),
    )
    .await
}

async fn transition(
    State(state): State<StateData>,
    Extension(request_id): Extension<RequestId>,
    Path(id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
    action: &'static str,
    (from, to): (&'static str, &'static str),
) -> Result<Response, AppError> {
    let value = body(payload, request_id)?;
    let route = format!("/api/v1/runs/{id}/{action}");
    mutate(
        &state,
        &headers,
        Method::POST,
        &route,
        &value,
        request_id,
        || async {
            let input: Transition = parse(&value, request_id)?;
            let expected = serde_json::to_value(input.expected_status)
                .map_err(|error| AppError::internal(request_id, error))?;
            if expected != json!(from) && !(action == "cancel" && expected == json!("PAUSED")) {
                return Err(AppError::conflict(request_id, Value::Null));
            }
            let key = canonical_uuid(&id, request_id)?;
            // ponytail: status compare-and-swap is concurrency guard until project_runs has a version column.
            let result = sqlx::query(
                "UPDATE project_runs SET status=$1,updated_at=now() WHERE id=$2 AND status=$3",
            )
            .bind(to)
            .bind(key)
            .bind(expected.as_str().unwrap_or(from))
            .execute(&state.pool)
            .await
            .map_err(|error| AppError::internal(request_id, error))?;
            if result.rows_affected() == 0 {
                return Err(AppError::conflict(request_id, Value::Null));
            }
            let run = state
                .store
                .get_run(&id)
                .await
                .map_err(|error| store_error(error, request_id))?;
            Ok((StatusCode::OK, json!({"run":run})))
        },
    )
    .await
}
