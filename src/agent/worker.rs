use std::{
    collections::BTreeSet,
    fmt,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    context::{BuiltContext, ContextBuilder, ContextError, ContextRequest},
    domain::{
        state_machine::{Actor, transition},
        task::{TaskContract, TaskStatus},
    },
    model::{
        Message, MessageRole, ModelLimits, ModelRequest, ModelResponse, ToolCall, ToolDefinition,
        Usage,
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
    pub output_tokens: u64,
    pub tool_calls: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCheckpoint {
    pub call_id: String,
    pub tool: String,
    pub succeeded: bool,
    pub error_code: Option<ToolErrorCode>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerError {
    Context,
    Model,
    InvalidModelResponse,
    Tool(ToolErrorCode),
    Checkpoint,
    InvalidTransition,
}

impl fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Context => "worker context build failed",
            Self::Model => "worker model call failed",
            Self::InvalidModelResponse => "worker model response is invalid",
            Self::Tool(_) => "worker tool call failed",
            Self::Checkpoint => "worker checkpoint persistence failed",
            Self::InvalidTransition => "worker handoff transition is invalid",
        })
    }
}
impl std::error::Error for WorkerError {}

pub trait WorkerModel {
    type Error;
    fn complete(&mut self, request: &ModelRequest) -> Result<ModelResponse, Self::Error>;
}

pub trait CheckpointStore {
    type Error;
    fn completed(&self, call_id: &str) -> Option<ToolCheckpoint>;
    fn save(&mut self, checkpoint: &ToolCheckpoint) -> Result<(), Self::Error>;
}

pub trait WorkerClock {
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

    pub fn run(mut self) -> WorkerRun<M, C> {
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
            let response = match call_model(
                &mut self.model,
                &request,
                self.contract.limits.max_attempts.get() as usize,
                self.clock.as_ref(),
                deadline,
            ) {
                Ok(response) => response,
                Err(stop) => {
                    let error = (stop == StopReason::ModelError).then_some(WorkerError::Model);
                    return self.finish(
                        summary,
                        None,
                        usage,
                        artifacts,
                        changed_paths,
                        stop,
                        error,
                    );
                }
            };
            if add_usage(&mut usage, response.usage, self.contract) {
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
                if let Some(checkpoint) = self.checkpoints.completed(&call.id) {
                    messages.push(tool_message(
                        &call.id,
                        checkpoint.succeeded,
                        checkpoint.error_code,
                    ));
                    continue;
                }
                usage.tool_calls += 1;
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
                let execution = self.tools.execute(request);
                let checkpoint = ToolCheckpoint {
                    call_id: call.id.clone(),
                    tool: call.name.clone(),
                    succeeded: execution.result.is_ok(),
                    error_code: execution.result.as_ref().err().map(|error| error.code),
                };
                if let Ok(result) = &execution.result {
                    collect_result(result, &mut artifacts, &mut changed_paths);
                }
                if self.checkpoints.save(&checkpoint).is_err() {
                    return self.finish(
                        summary,
                        None,
                        usage,
                        artifacts,
                        changed_paths,
                        StopReason::ToolError,
                        Some(WorkerError::Checkpoint),
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
                    checkpoint.succeeded,
                    checkpoint.error_code,
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

fn call_model<M: WorkerModel>(
    model: &mut M,
    request: &ModelRequest,
    attempts: usize,
    clock: &dyn WorkerClock,
    deadline: Duration,
) -> Result<ModelResponse, StopReason> {
    for _ in 0..attempts {
        if clock.elapsed() >= deadline {
            return Err(StopReason::Timeout);
        }
        let response = model.complete(request);
        if clock.elapsed() >= deadline {
            return Err(StopReason::Timeout);
        }
        if let Ok(response) = response {
            return Ok(response);
        }
    }
    Err(StopReason::ModelError)
}

fn add_usage(total: &mut TokenUsage, usage: Usage, contract: &TaskContract) -> bool {
    total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
    total.input_tokens > limit(contract.limits.max_input_tokens.get())
        || total.output_tokens > limit(contract.limits.max_output_tokens.get())
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
