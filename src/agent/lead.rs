use serde_json::json;

use crate::{
    api::contracts::plan::ProposedPlanInput,
    context::discovery::RepositoryMap,
    domain::{
        project::{ProjectRun, ProposedPlan},
        task::ValidationError,
    },
    model::{
        FinishReason, Message, MessageRole, ModelError, ModelErrorKind, ModelLimits, ModelRequest,
        ModelResponse,
    },
};

const MAX_INPUT_BYTES: usize = 48 * 1024;
const MAX_RESPONSE_BYTES: usize = 128 * 1024;
const INSTRUCTIONS: &str = include_str!("prompts/lead.md");

#[derive(Debug, PartialEq, Eq)]
pub enum LeadError {
    InvalidBrief(ValidationError),
    InputTooLarge,
    Model(ModelErrorKind),
    InvalidResponse,
    InvalidPlan(ValidationError),
    ProjectMismatch,
    UnsafeScope,
    OverlappingScope,
    UnknownVerification,
    BudgetExceeded,
}

impl std::fmt::Display for LeadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidBrief(_) => "invalid project brief",
            Self::InputTooLarge => "lead input exceeds limit",
            Self::Model(_) => "lead model call failed",
            Self::InvalidResponse => "lead response is invalid",
            Self::InvalidPlan(_) => "lead plan is invalid",
            Self::ProjectMismatch => "lead plan project mismatch",
            Self::UnsafeScope => "lead plan contains unsafe file scope",
            Self::OverlappingScope => "lead plan contains overlapping file scopes",
            Self::UnknownVerification => "lead plan contains unapproved verification command",
            Self::BudgetExceeded => "lead plan exceeds run token budget",
        })
    }
}

impl std::error::Error for LeadError {}

#[allow(async_fn_in_trait)]
pub trait LeadModel {
    async fn complete(&mut self, request: &ModelRequest) -> Result<ModelResponse, ModelError>;
}

impl LeadModel for crate::openai::OpenAiToolsClient {
    async fn complete(&mut self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        crate::openai::OpenAiToolsClient::complete(self, request).await
    }
}

/// Menghasilkan proposal saja; penyimpanan dan persetujuan tetap milik control plane.
pub async fn propose<M: LeadModel>(
    model: &mut M,
    run: &ProjectRun,
    discovery: &RepositoryMap,
    agent_run_id: &str,
    limits: ModelLimits,
) -> Result<ProposedPlan, LeadError> {
    run.validate().map_err(LeadError::InvalidBrief)?;
    if agent_run_id.trim().is_empty()
        || limits.max_input_tokens == 0
        || limits.max_output_tokens == 0
        || discovery.test_commands.is_empty()
    {
        return Err(LeadError::InvalidResponse);
    }
    let content = serde_json::to_string(&json!({
        "project_id": run.project_id,
        "run_id": run.id,
        "objective": run.objective,
        "acceptance_criteria": run.acceptance_criteria,
        "token_budget": run.token_budget,
        "discovery": discovery,
    }))
    .map_err(|_| LeadError::InputTooLarge)?;
    let input_bytes = content.len() + INSTRUCTIONS.len();
    if input_bytes > MAX_INPUT_BYTES || input_bytes.div_ceil(4) as u64 > limits.max_input_tokens {
        return Err(LeadError::InputTooLarge);
    }
    let response = model
        .complete(&ModelRequest {
            project_id: run.project_id.as_str().to_owned(),
            task_id: run.id.as_str().to_owned(),
            agent_run_id: agent_run_id.to_owned(),
            model_class: "reasoning".to_owned(),
            messages: vec![
                Message {
                    role: MessageRole::System,
                    content: INSTRUCTIONS.to_owned(),
                    tool_call_id: None,
                },
                Message {
                    role: MessageRole::User,
                    content,
                    tool_call_id: None,
                },
            ],
            tools: Vec::new(),
            limits,
        })
        .await
        .map_err(|error| LeadError::Model(error.kind()))?;
    parse_plan(response, run, discovery)
}

fn parse_plan(
    response: ModelResponse,
    run: &ProjectRun,
    discovery: &RepositoryMap,
) -> Result<ProposedPlan, LeadError> {
    if response.finish_reason != FinishReason::Stop || !response.tool_calls.is_empty() {
        return Err(LeadError::InvalidResponse);
    }
    let body = response.content.ok_or(LeadError::InvalidResponse)?;
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(LeadError::InvalidResponse);
    }
    let mut input: ProposedPlanInput =
        serde_json::from_str(&body).map_err(|_| LeadError::InvalidResponse)?;
    // `context_refs` hanya boleh berisi `artifact://<id>`; model nyata sering mengisinya dengan path file, yang membuat
    // setiap dispatch gagal (`orchestrator.context`) SETELAH manusia menyetujui plan. Akses file tidak berasal dari sini
    // (itu `allowed_paths`), jadi referensi lain dibuang saat parse, bukan dibiarkan merusak eksekusi.
    for task in &mut input.tasks {
        task.context_refs
            .retain(|reference| reference.starts_with("artifact://"));
    }
    let plan = ProposedPlan::try_from(input).map_err(LeadError::InvalidPlan)?;
    if plan.project_run_id != run.id
        || plan
            .tasks
            .iter()
            .any(|task| task.project_id != run.project_id)
    {
        return Err(LeadError::ProjectMismatch);
    }

    let mut scopes: Vec<&str> = Vec::new();
    let mut reserved = 0_i64;
    for task in &plan.tasks {
        // ponytail: reservation counts every attempt at maximum input+output; reconcile actual usage in scheduler.
        let tokens = task
            .limits
            .max_input_tokens
            .get()
            .checked_add(task.limits.max_output_tokens.get())
            .and_then(|n| n.checked_mul(task.limits.max_attempts.get()))
            .ok_or(LeadError::BudgetExceeded)?;
        reserved = reserved
            .checked_add(tokens)
            .ok_or(LeadError::BudgetExceeded)?;
        if reserved > run.token_budget.get() {
            return Err(LeadError::BudgetExceeded);
        }
        for command in &task.verification_commands {
            if !discovery
                .test_commands
                .iter()
                .any(|known| known == command.as_str())
            {
                return Err(LeadError::UnknownVerification);
            }
        }
        for path in &task.allowed_paths {
            let scope = path.as_str();
            if !safe_scope(scope) {
                return Err(LeadError::UnsafeScope);
            }
            if scopes.iter().any(|prior| overlaps(prior, scope)) {
                return Err(LeadError::OverlappingScope);
            }
            scopes.push(scope);
        }
    }
    Ok(plan)
}

fn safe_scope(scope: &str) -> bool {
    let scope = scope
        .strip_suffix("/**")
        .or_else(|| scope.strip_suffix("/*"))
        .unwrap_or(scope);
    !scope.is_empty()
        && scope != "*"
        && !scope.contains(['*', '?', '[', ']', '{', '}', '\\'])
        && !scope.split('/').any(|part| {
            part.is_empty()
                || part == ".git"
                || part == ".env"
                || part.starts_with(".env.")
                || part == ".."
        })
}

fn overlaps(left: &str, right: &str) -> bool {
    fn covers(pattern: &str, path: &str) -> bool {
        if let Some(prefix) = pattern.strip_suffix("/**") {
            path == prefix || path.starts_with(&format!("{prefix}/"))
        } else if let Some(prefix) = pattern.strip_suffix("/*") {
            path.strip_prefix(prefix)
                .and_then(|rest| rest.strip_prefix('/'))
                .is_some_and(|rest| !rest.is_empty() && !rest.contains('/'))
        } else {
            pattern == path
        }
    }
    // ponytail: conservative scope lock rejects even sequential editors; allow shared paths after file leases exist.
    let l = left.trim_end_matches("/**").trim_end_matches("/*");
    let r = right.trim_end_matches("/**").trim_end_matches("/*");
    covers(left, r)
        || covers(right, l)
        || (left.ends_with("/**") && r.starts_with(&format!("{l}/")))
        || (right.ends_with("/**") && l.starts_with(&format!("{r}/")))
        || (left.ends_with("/*") && right.ends_with("/*") && l == r)
}
