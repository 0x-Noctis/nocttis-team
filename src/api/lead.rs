//! `POST /api/v1/runs/{id}/lead-plan`: meminta Lead Agent menyusun plan dari brief run + discovery repository.
//! Hasilnya disimpan sebagai plan `PROPOSED`; persetujuan tetap lewat `approve-plan` oleh manusia.

use std::{collections::HashSet, env, time::Duration};

use axum::{
    Json,
    extract::{Extension, Path, State, rejection::JsonRejection},
    http::{HeaderMap, Method, StatusCode},
    response::Response,
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    AppError, RequestId,
    projects::{StateData, body, mutate, parse, store_error},
};
use crate::{
    agent::lead::{self, LeadError},
    context::discovery::discover,
    domain::{
        project::{PlanStatus, ProposedPlan, RunStatus},
        task::{NonEmptyString, PositiveLimit},
    },
    model::{ModelErrorKind, ModelLimits},
    openai::OpenAiToolsClient,
    store::provider::{ProviderRepository, StoreError},
};

// Batas output Lead: plan JSON jarang lebih dari beberapa ribu token.
const MAX_LEAD_OUTPUT_TOKENS: u64 = 16_384;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LeadPlanInput {
    model_id: Option<String>,
}

pub(crate) async fn lead_plan(
    State(state): State<StateData>,
    Extension(request_id): Extension<RequestId>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<Response, AppError> {
    let value = body(payload, request_id)?;
    let route = format!("/api/v1/runs/{run_id}/lead-plan");
    mutate(
        &state,
        &headers,
        Method::POST,
        &route,
        &value,
        request_id,
        || async {
            let input: LeadPlanInput = parse(&value, request_id)?;
            let model_id = input
                .model_id
                .or_else(|| state.default_model.clone())
                .ok_or_else(|| {
                    AppError::unprocessable(
                        request_id,
                        json!({"model_id":"required: no default model is configured"}),
                    )
                })?;
            let run = state
                .store
                .get_run(&run_id)
                .await
                .map_err(|error| store_error(error, request_id))?;
            if !matches!(run.status, RunStatus::Planning | RunStatus::AwaitingApproval) {
                return Err(AppError::conflict(
                    request_id,
                    json!({"run":"status does not allow planning"}),
                ));
            }
            let existing = state
                .store
                .list_plans(&run_id)
                .await
                .map_err(|error| store_error(error, request_id))?;
            if existing
                .iter()
                .any(|plan| plan.status == PlanStatus::Proposed)
            {
                return Err(AppError::conflict(
                    request_id,
                    json!({"plan":"a proposed plan is already awaiting a decision"}),
                ));
            }
            // Plans diurutkan terbaru dulu, jadi versi berikutnya = versi pertama + 1.
            let version = existing.first().map_or(1, |plan| plan.version.get() + 1);

            let project = state
                .store
                .get_project(run.project_id.as_str())
                .await
                .map_err(|error| store_error(error, request_id))?;
            let repository = project.repository_path;
            let map = tokio::task::spawn_blocking(move || discover(repository.as_path()))
                .await
                .map_err(|error| AppError::internal(request_id, error))?
                .map_err(|_| {
                    AppError::unprocessable(request_id, json!({"repository":"discovery failed"}))
                })?;
            if map.test_commands.is_empty() {
                return Err(AppError::unprocessable(
                    request_id,
                    json!({"repository":"no test commands detected; the Lead can only plan verifiable tasks"}),
                ));
            }

            let providers = ProviderRepository::new(state.pool.clone());
            let model = providers.get_model(&model_id).await.map_err(|error| match error {
                StoreError::NotFound => AppError::unprocessable(
                    request_id,
                    json!({"model_id":"model is not registered"}),
                ),
                other => AppError::internal(request_id, std::io::Error::other(other.to_string())),
            })?;
            let provider = providers
                .get_provider(model.provider_id.as_str())
                .await
                .map_err(|error| AppError::internal(request_id, std::io::Error::other(error.to_string())))?
                .provider;
            // Nilai secret hanya dipakai untuk membangun client; tidak pernah masuk log atau respons.
            let secret = env::var(provider.api_key_env.as_str()).map_err(|_| {
                AppError::conflict(request_id, json!({"provider":"secret is not configured"}))
            })?;
            let mut client = OpenAiToolsClient::new(
                provider.base_url.as_str(),
                secret,
                model.remote_name.as_str(),
                Duration::from_secs(provider.request_timeout_seconds.get() as u64),
            )
            .map_err(|_| AppError::bad_gateway(request_id, json!({"lead":"model client unavailable"})))?;

            let max_output = (model.max_output_tokens.get() as u64).min(MAX_LEAD_OUTPUT_TOKENS);
            let context = model.context_window.get() as u64;
            let limits = ModelLimits {
                max_input_tokens: context.saturating_sub(max_output).max(1),
                max_output_tokens: max_output,
            };
            // Lead dipanggil di luar tabel agent_runs, jadi ID ini hanya untuk korelasi request.
            // ponytail: usage Lead belum dicatat ke model_usage; catat saat budget guard (M4-003) siap.
            let agent_run_id = Uuid::new_v4().to_string();
            let mut plan = lead::propose(&mut client, &run, &map, &agent_run_id, limits)
                .await
                .map_err(|error| lead_error(error, request_id))?;
            namespace_plan(&mut plan, &run_id, version, request_id)?;
            state
                .store
                .propose(&plan)
                .await
                .map_err(|error| store_error(error, request_id))?;
            Ok((StatusCode::CREATED, json!({"plan": plan})))
        },
    )
    .await
}

fn lead_error(error: LeadError, request_id: RequestId) -> AppError {
    match error {
        LeadError::Model(ModelErrorKind::RateLimited) => {
            AppError::rate_limited(request_id, json!({"lead":"model provider rate limited"}))
        }
        LeadError::Model(ModelErrorKind::ContextTooLarge) => AppError::unprocessable(
            request_id,
            json!({"lead":"brief exceeds the model context"}),
        ),
        LeadError::Model(_) => {
            AppError::bad_gateway(request_id, json!({"lead":"model provider failed"}))
        }
        // Pesan Display LeadError berupa konstanta aman (tanpa isi respons model).
        other => AppError::unprocessable(request_id, json!({"lead": other.to_string()})),
    }
}

/// Model memilih ID plan/task sendiri ("plan-1", "task-1"), padahal ID task unik global.
/// Beri prefix run dan paksa versi/ID plan sesuai urutan di database supaya dua run tidak bertabrakan.
fn namespace_plan(
    plan: &mut ProposedPlan,
    run_id: &str,
    version: i64,
    request_id: RequestId,
) -> Result<(), AppError> {
    let invalid = || {
        AppError::unprocessable(
            request_id,
            json!({"lead":"lead plan has unusable task IDs"}),
        )
    };
    let prefix: String = run_id.chars().take(8).collect();
    let rename = |old: &str| {
        let safe: String = old
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        format!("{prefix}-{safe}")
    };
    let mapping: Vec<(String, String)> = plan
        .tasks
        .iter()
        .map(|task| (task.id.as_str().to_owned(), rename(task.id.as_str())))
        .collect();
    // Sanitasi bisa membuat dua ID berbeda menjadi sama; tolak daripada menimpa diam-diam.
    if mapping
        .iter()
        .map(|(_, new)| new)
        .collect::<HashSet<_>>()
        .len()
        != mapping.len()
    {
        return Err(invalid());
    }
    let lookup = |old: &str| {
        mapping
            .iter()
            .find(|(from, _)| from == old)
            .map(|(_, to)| to.clone())
    };
    for task in &mut plan.tasks {
        task.id = NonEmptyString::parse("id", lookup(task.id.as_str()).ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        task.depends_on = task
            .depends_on
            .iter()
            .map(|dep| {
                let new = lookup(dep.as_str()).ok_or_else(invalid)?;
                NonEmptyString::parse("depends_on", new).map_err(|_| invalid())
            })
            .collect::<Result<_, _>>()?;
    }
    plan.id =
        NonEmptyString::parse("id", format!("plan-{run_id}-v{version}")).map_err(|_| invalid())?;
    plan.version = PositiveLimit::new("version", version).map_err(|_| invalid())?;
    plan.validate().map_err(|_| invalid())
}
