use std::{
    collections::BTreeSet,
    fmt,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    context::{BuiltContext, ContextBuilder, ContextError, ContextRequest},
    domain::{
        state_machine::{Actor, transition},
        task::{TaskContract, TaskStatus},
    },
    model::{
        Message, MessageRole, ModelError, ModelErrorKind, ModelLimits, ModelRequest, ModelResponse,
        ToolCall, ToolDefinition,
    },
    runner::tools::{StructuredTools, ToolErrorCode, ToolRequest, ToolResult},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Completed,
    BudgetExhausted,
    MaxTurns,
    Timeout,
    ModelError,
    ToolError,
    RecoveryRequired,
    UsageOverflow,
    HumanRequested,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkerHandoff {
    pub summary: String,
    pub next_status: Option<TaskStatus>,
    pub changed_paths: Vec<String>,
    pub verification_commands: Vec<String>,
    pub artifacts: Vec<String>,
    pub token_usage: TokenUsage,
    pub stop_reason: StopReason,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub cached_tokens: u64,
    pub output_tokens: u64,
    pub tool_calls: u64,
    pub latency_ms: u128,
    pub estimated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCheckpoint {
    pub call_id: String,
    pub outcome: CheckpointOutcome,
    pub duration_ms: u64,
    pub artifact_id: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckpointOutcome {
    Succeeded,
    Failed,
    TimedOut,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckpointReservation {
    New,
    Completed(ToolCheckpoint),
    InProgress,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerError {
    Context,
    Model(ModelErrorKind),
    InvalidModelResponse,
    Tool(ToolErrorCode),
    CheckpointReservation,
    CheckpointCompletion,
    AmbiguousToolCall,
    UsageOverflow,
    InvalidTransition,
}

impl fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Context => "worker context build failed",
            Self::Model(_) => "worker model call failed",
            Self::InvalidModelResponse => "worker model response is invalid",
            Self::Tool(_) => "worker tool call failed",
            Self::CheckpointReservation => "worker checkpoint reservation failed",
            Self::CheckpointCompletion => "worker checkpoint completion failed",
            Self::AmbiguousToolCall => "worker tool call outcome requires recovery",
            Self::UsageOverflow => "worker usage overflow",
            Self::InvalidTransition => "worker handoff transition is invalid",
        })
    }
}
impl std::error::Error for WorkerError {}

impl WorkerModel for crate::openai::OpenAiToolsClient {
    async fn complete(&mut self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        crate::openai::OpenAiToolsClient::complete(self, request).await
    }
}

#[allow(async_fn_in_trait)]
pub trait WorkerModel {
    async fn complete(&mut self, request: &ModelRequest) -> Result<ModelResponse, ModelError>;

    /// True bila model sudah mengulang/berpindah sendiri (mis. `ModelRouter`); worker lalu tidak mengulang lagi supaya
    /// jumlah percobaan tidak berlipat.
    fn handles_retries(&self) -> bool {
        false
    }
}

impl crate::model::router::CompletionModel for crate::openai::OpenAiToolsClient {
    async fn complete(&mut self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        crate::openai::OpenAiToolsClient::complete(self, request).await
    }
}

impl<M, S, G> WorkerModel for crate::model::router::ModelRouter<M, S, G>
where
    M: crate::model::router::CompletionModel,
    S: crate::model::retry::Sleeper,
    G: crate::model::router::CallGuard,
{
    async fn complete(&mut self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        crate::model::router::ModelRouter::complete(self, request).await
    }

    fn handles_retries(&self) -> bool {
        true
    }
}

#[allow(async_fn_in_trait)]
pub trait CheckpointStore {
    type Error;
    async fn reserve(
        &mut self,
        agent_run_id: &str,
        call_id: &str,
        tool: &str,
    ) -> Result<CheckpointReservation, Self::Error>;
    async fn complete(
        &mut self,
        agent_run_id: &str,
        checkpoint: &ToolCheckpoint,
    ) -> Result<(), Self::Error>;
}

pub trait WorkerClock: Send + Sync {
    fn elapsed(&self) -> Duration;
}

struct SystemClock(Instant);

impl WorkerClock for SystemClock {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
}

#[derive(Clone, Debug)]
pub struct WorkerConfig {
    pub agent_run_id: String,
    pub model_class: String,
    pub max_turns: u32,
    pub deadline: Duration,
}

pub struct Worker<'a, M, C> {
    contract: &'a TaskContract,
    status: TaskStatus,
    context: &'a ContextBuilder<'a>,
    tools: &'a StructuredTools<'a>,
    model: M,
    checkpoints: C,
    config: WorkerConfig,
    clock: Box<dyn WorkerClock + 'a>,
}

impl<'a, M: WorkerModel, C: CheckpointStore> Worker<'a, M, C> {
    pub fn new(
        contract: &'a TaskContract,
        status: TaskStatus,
        context: &'a ContextBuilder<'a>,
        tools: &'a StructuredTools<'a>,
        model: M,
        checkpoints: C,
        config: WorkerConfig,
    ) -> Self {
        Self::with_clock(
            contract,
            status,
            context,
            tools,
            model,
            checkpoints,
            config,
            SystemClock(Instant::now()),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_clock(
        contract: &'a TaskContract,
        status: TaskStatus,
        context: &'a ContextBuilder<'a>,
        tools: &'a StructuredTools<'a>,
        model: M,
        checkpoints: C,
        config: WorkerConfig,
        clock: impl WorkerClock + 'a,
    ) -> Self {
        Self {
            contract,
            status,
            context,
            tools,
            model,
            checkpoints,
            config,
            clock: Box::new(clock),
        }
    }

    pub async fn run(mut self) -> WorkerRun<M, C> {
        let deadline = self.config.deadline.min(Duration::from_secs(
            self.contract.limits.timeout_seconds.get() as u64,
        ));
        let mut messages = Vec::new();
        let mut usage = TokenUsage::default();
        let mut artifacts = BTreeSet::new();
        let mut changed_paths = BTreeSet::new();
        let mut context_dirty = true;
        let mut summary = String::new();

        for _ in 0..self.config.max_turns {
            if self.deadline_reached(deadline) {
                return self.finish(
                    summary,
                    None,
                    usage,
                    artifacts,
                    changed_paths,
                    StopReason::Timeout,
                    None,
                );
            }
            if context_dirty {
                match self
                    .context
                    .build(self.contract, &ContextRequest::default())
                {
                    Ok(context) => messages.push(context_message(&context)),
                    Err(_) => {
                        return self.finish(
                            summary,
                            None,
                            usage,
                            artifacts,
                            changed_paths,
                            StopReason::ModelError,
                            Some(WorkerError::Context),
                        );
                    }
                }
                context_dirty = false;
            }
            let remaining_input = limit(self.contract.limits.max_input_tokens.get())
                .saturating_sub(usage.input_tokens);
            let remaining_output = limit(self.contract.limits.max_output_tokens.get())
                .saturating_sub(usage.output_tokens);
            if remaining_input == 0 || remaining_output == 0 {
                return self.finish(
                    summary,
                    None,
                    usage,
                    artifacts,
                    changed_paths,
                    StopReason::BudgetExhausted,
                    None,
                );
            }
            let request = model_request(
                self.contract,
                &self.config,
                messages.clone(),
                remaining_input,
                remaining_output,
            );
            let model_attempts = if self.model.handles_retries() {
                1
            } else {
                self.contract.limits.max_attempts.get() as usize
            };
            let response = match call_model(
                &mut self.model,
                &request,
                model_attempts,
                self.clock.as_ref(),
                deadline,
            )
            .await
            {
                Ok(response) => response,
                Err(CallModelError::Timeout) => {
                    return self.finish(
                        summary,
                        None,
                        usage,
                        artifacts,
                        changed_paths,
                        StopReason::Timeout,
                        None,
                    );
                }
                Err(CallModelError::Model(kind)) => {
                    return self.finish(
                        summary,
                        None,
                        usage,
                        artifacts,
                        changed_paths,
                        StopReason::ModelError,
                        Some(WorkerError::Model(kind)),
                    );
                }
            };
            match add_usage(&mut usage, &response, self.contract) {
                Ok(false) => {}
                Ok(true) => {
                    return self.finish(
                        summary,
                        None,
                        usage,
                        artifacts,
                        changed_paths,
                        StopReason::BudgetExhausted,
                        None,
                    );
                }
                Err(error) => {
                    return self.finish(
                        summary,
                        None,
                        usage,
                        artifacts,
                        changed_paths,
                        StopReason::UsageOverflow,
                        Some(error),
                    );
                }
            }
            if let Some(text) = response
                .content
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
            {
                messages.push(Message {
                    role: MessageRole::Assistant,
                    content: text.to_owned(),
                    tool_call_id: None,
                });
            }
            if response.tool_calls.is_empty() {
                let completion = match parse_completion(response.content.as_deref()) {
                    Ok(value) => value,
                    Err(error) => {
                        return self.finish(
                            summary,
                            None,
                            usage,
                            artifacts,
                            changed_paths,
                            StopReason::ModelError,
                            Some(error),
                        );
                    }
                };
                summary = completion.summary;
                let next = match completion.status {
                    HandoffStatus::SelfCheck => TaskStatus::SelfCheck,
                    HandoffStatus::Review => TaskStatus::Review,
                };
                if transition(self.contract, self.status, next, Actor::Worker).is_err() {
                    return self.finish(
                        summary,
                        None,
                        usage,
                        artifacts,
                        changed_paths,
                        StopReason::ModelError,
                        Some(WorkerError::InvalidTransition),
                    );
                }
                return self.finish(
                    summary,
                    Some(next),
                    usage,
                    artifacts,
                    changed_paths,
                    StopReason::Completed,
                    None,
                );
            }
            for call in response.tool_calls {
                if usage.tool_calls >= limit(self.contract.limits.max_tool_calls.get()) {
                    return self.finish(
                        summary,
                        None,
                        usage,
                        artifacts,
                        changed_paths,
                        StopReason::BudgetExhausted,
                        None,
                    );
                }
                if self.deadline_reached(deadline) {
                    return self.finish(
                        summary,
                        None,
                        usage,
                        artifacts,
                        changed_paths,
                        StopReason::Timeout,
                        None,
                    );
                }
                let request = match parse_tool_call(&call) {
                    Ok(request) => request,
                    Err(error) => {
                        return self.finish(
                            summary,
                            None,
                            usage,
                            artifacts,
                            changed_paths,
                            StopReason::ToolError,
                            Some(error),
                        );
                    }
                };
                match self
                    .checkpoints
                    .reserve(&self.config.agent_run_id, &call.id, &call.name)
                    .await
                {
                    Ok(CheckpointReservation::Completed(checkpoint)) => {
                        messages.push(tool_message(
                            &call.id,
                            checkpoint.outcome == CheckpointOutcome::Succeeded,
                            match checkpoint.outcome {
                                CheckpointOutcome::Succeeded => None,
                                CheckpointOutcome::Failed => Some(ToolErrorCode::OperationFailed),
                                CheckpointOutcome::TimedOut => Some(ToolErrorCode::Timeout),
                            },
                        ));
                        continue;
                    }
                    Ok(CheckpointReservation::InProgress) => {
                        return self.finish(
                            summary,
                            None,
                            usage,
                            artifacts,
                            changed_paths,
                            StopReason::RecoveryRequired,
                            Some(WorkerError::AmbiguousToolCall),
                        );
                    }
                    Ok(CheckpointReservation::New) => {}
                    Err(_) => {
                        return self.finish(
                            summary,
                            None,
                            usage,
                            artifacts,
                            changed_paths,
                            StopReason::ToolError,
                            Some(WorkerError::CheckpointReservation),
                        );
                    }
                }
                usage.tool_calls = match usage.tool_calls.checked_add(1) {
                    Some(value) => value,
                    None => {
                        return self.finish(
                            summary,
                            None,
                            usage,
                            artifacts,
                            changed_paths,
                            StopReason::UsageOverflow,
                            Some(WorkerError::UsageOverflow),
                        );
                    }
                };
                let artifact_id = match &request {
                    ToolRequest::SubmitArtifact { artifact_id, .. } => {
                        Uuid::parse_str(artifact_id).ok()
                    }
                    _ => None,
                };
                let execution = self.tools.execute(request);
                let checkpoint = ToolCheckpoint {
                    call_id: call.id.clone(),
                    outcome: match execution.result.as_ref().err().map(|error| error.code) {
                        None => CheckpointOutcome::Succeeded,
                        Some(ToolErrorCode::Timeout) => CheckpointOutcome::TimedOut,
                        Some(_) => CheckpointOutcome::Failed,
                    },
                    duration_ms: execution.audit.duration_ms,
                    artifact_id,
                };
                if let Ok(result) = &execution.result {
                    collect_result(result, &mut artifacts, &mut changed_paths);
                }
                if self
                    .checkpoints
                    .complete(&self.config.agent_run_id, &checkpoint)
                    .await
                    .is_err()
                {
                    return self.finish(
                        summary,
                        None,
                        usage,
                        artifacts,
                        changed_paths,
                        StopReason::ToolError,
                        Some(WorkerError::CheckpointCompletion),
                    );
                }
                if self.deadline_reached(deadline) {
                    return self.finish(
                        summary,
                        None,
                        usage,
                        artifacts,
                        changed_paths,
                        StopReason::Timeout,
                        None,
                    );
                }
                if matches!(execution.result, Ok(ToolResult::PatchApplied)) {
                    let status = self.tools.execute(ToolRequest::GitStatus);
                    if let Ok(result) = &status.result {
                        collect_result(result, &mut artifacts, &mut changed_paths);
                    }
                }
                messages.push(tool_message(
                    &call.id,
                    checkpoint.outcome == CheckpointOutcome::Succeeded,
                    execution.result.as_ref().err().map(|error| error.code),
                ));
                context_dirty = true;
                match execution.result {
                    Ok(ToolResult::HumanRequested { message }) => {
                        return self.finish(
                            message,
                            None,
                            usage,
                            artifacts,
                            changed_paths,
                            StopReason::HumanRequested,
                            None,
                        );
                    }
                    Err(error_value) => {
                        return self.finish(
                            summary,
                            None,
                            usage,
                            artifacts,
                            changed_paths,
                            StopReason::ToolError,
                            Some(WorkerError::Tool(error_value.code)),
                        );
                    }
                    _ => {}
                }
            }
        }
        self.finish(
            summary,
            None,
            usage,
            artifacts,
            changed_paths,
            StopReason::MaxTurns,
            None,
        )
    }

    fn deadline_reached(&self, deadline: Duration) -> bool {
        self.clock.elapsed() >= deadline
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        self,
        summary: String,
        next_status: Option<TaskStatus>,
        token_usage: TokenUsage,
        artifacts: BTreeSet<String>,
        changed_paths: BTreeSet<String>,
        stop_reason: StopReason,
        error: Option<WorkerError>,
    ) -> WorkerRun<M, C> {
        WorkerRun {
            handoff: WorkerHandoff {
                summary,
                next_status,
                changed_paths: changed_paths.into_iter().collect(),
                verification_commands: self
                    .contract
                    .verification_commands
                    .iter()
                    .map(|value| value.as_str().to_owned())
                    .collect(),
                artifacts: artifacts.into_iter().collect(),
                token_usage,
                stop_reason,
            },
            error,
            model: self.model,
            checkpoints: self.checkpoints,
        }
    }
}

pub struct WorkerRun<M, C> {
    pub handoff: WorkerHandoff,
    pub error: Option<WorkerError>,
    pub model: M,
    pub checkpoints: C,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Completion {
    summary: String,
    status: HandoffStatus,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum HandoffStatus {
    SelfCheck,
    Review,
}

fn parse_completion(content: Option<&str>) -> Result<Completion, WorkerError> {
    serde_json::from_str(content.ok_or(WorkerError::InvalidModelResponse)?)
        .map_err(|_| WorkerError::InvalidModelResponse)
}

fn context_message(context: &BuiltContext) -> Message {
    Message {
        role: MessageRole::User,
        content: context
            .entries
            .iter()
            .map(|entry| entry.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n"),
        tool_call_id: None,
    }
}

fn model_request(
    contract: &TaskContract,
    config: &WorkerConfig,
    messages: Vec<Message>,
    input: u64,
    output: u64,
) -> ModelRequest {
    ModelRequest {
        project_id: contract.project_id.as_str().to_owned(),
        task_id: contract.id.as_str().to_owned(),
        agent_run_id: config.agent_run_id.clone(),
        model_class: config.model_class.clone(),
        messages,
        tools: tool_definitions(),
        limits: ModelLimits {
            max_input_tokens: input,
            max_output_tokens: output,
        },
    }
}

fn tool_definitions() -> Vec<ToolDefinition> {
    [
        "list_files",
        "search_code",
        "read_file",
        "apply_patch",
        "git_diff",
        "git_status",
        "submit_artifact",
        "request_human",
    ]
    .into_iter()
    .map(|name| ToolDefinition {
        name: name.to_owned(),
        description: format!("Structured {name} tool"),
        input_schema: json!({"type":"object"}),
    })
    .collect()
}

enum CallModelError {
    Timeout,
    Model(ModelErrorKind),
}

async fn call_model<M: WorkerModel>(
    model: &mut M,
    request: &ModelRequest,
    attempts: usize,
    clock: &dyn WorkerClock,
    deadline: Duration,
) -> Result<ModelResponse, CallModelError> {
    let mut last_error = ModelErrorKind::ProviderUnavailable;
    for _ in 0..attempts {
        if clock.elapsed() >= deadline {
            return Err(CallModelError::Timeout);
        }
        let response = model.complete(request).await;
        if clock.elapsed() >= deadline {
            return Err(CallModelError::Timeout);
        }
        match response {
            Ok(response) => return Ok(response),
            Err(error) => {
                last_error = error.kind();
                // Auth/konteks/respons tak valid/budget tidak berubah dengan diulang; hentikan seketika.
                if !error.retryable() {
                    break;
                }
            }
        }
    }
    Err(CallModelError::Model(last_error))
}

fn add_usage(
    total: &mut TokenUsage,
    response: &ModelResponse,
    contract: &TaskContract,
) -> Result<bool, WorkerError> {
    let input_tokens = total
        .input_tokens
        .checked_add(response.usage.input_tokens)
        .ok_or(WorkerError::UsageOverflow)?;
    let cached_tokens = total
        .cached_tokens
        .checked_add(response.usage.cached_tokens)
        .ok_or(WorkerError::UsageOverflow)?;
    let output_tokens = total
        .output_tokens
        .checked_add(response.usage.output_tokens)
        .ok_or(WorkerError::UsageOverflow)?;
    let latency_ms = total
        .latency_ms
        .checked_add(response.latency_ms)
        .ok_or(WorkerError::UsageOverflow)?;
    *total = TokenUsage {
        input_tokens,
        cached_tokens,
        output_tokens,
        tool_calls: total.tool_calls,
        latency_ms,
        estimated: total.estimated || response.usage.estimated,
    };
    Ok(input_tokens > limit(contract.limits.max_input_tokens.get())
        || output_tokens > limit(contract.limits.max_output_tokens.get()))
}
fn limit(value: i64) -> u64 {
    value.try_into().unwrap_or(0)
}

fn parse_tool_call(call: &ToolCall) -> Result<ToolRequest, WorkerError> {
    let string = |key: &str| {
        call.arguments
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(WorkerError::InvalidModelResponse)
    };
    match call.name.as_str() {
        "list_files" => Ok(ToolRequest::ListFiles {
            path: string("path")?,
        }),
        "search_code" => Ok(ToolRequest::SearchCode {
            path: string("path")?,
            query: string("query")?,
        }),
        "read_file" => Ok(ToolRequest::ReadFile {
            path: string("path")?,
        }),
        "apply_patch" => Ok(ToolRequest::ApplyPatch {
            patch: string("patch")?.into_bytes(),
        }),
        "git_diff" => Ok(ToolRequest::GitDiff),
        "git_status" => Ok(ToolRequest::GitStatus),
        "submit_artifact" => Ok(ToolRequest::SubmitArtifact {
            path: string("path")?,
            artifact_id: string("artifact_id")?,
            logical_name: string("logical_name")?,
            media_type: string("media_type")?,
        }),
        "request_human" => Ok(ToolRequest::RequestHuman {
            message: string("message")?,
        }),
        _ => Err(WorkerError::InvalidModelResponse),
    }
}

fn tool_message(call_id: &str, succeeded: bool, code: Option<ToolErrorCode>) -> Message {
    Message {
        role: MessageRole::Tool,
        content: serde_json::to_string(
            &json!({"succeeded": succeeded, "error_code": code.map(|value| format!("{value:?}"))}),
        )
        .unwrap_or_else(|_| "{}".to_owned()),
        tool_call_id: Some(call_id.to_owned()),
    }
}

fn collect_result(
    result: &ToolResult,
    artifacts: &mut BTreeSet<String>,
    changed_paths: &mut BTreeSet<String>,
) {
    match result {
        ToolResult::Artifact(metadata) => {
            artifacts.insert(metadata.artifact_id.clone());
        }
        ToolResult::Status(status) => {
            for line in status.lines() {
                if let Some(path) = line.get(3..) {
                    changed_paths.insert(path.trim().to_owned());
                }
            }
        }
        _ => {}
    }
}

impl From<ContextError> for WorkerError {
    fn from(_: ContextError) -> Self {
        Self::Context
    }
}
