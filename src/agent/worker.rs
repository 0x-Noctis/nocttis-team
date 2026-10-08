use std::{
    collections::BTreeSet,
    fmt,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    context::{BuiltContext, ContextBuilder, ContextError, ContextRequest, ContextSourceKind},
    domain::{
        state_machine::{Actor, transition},
        task::{TaskContract, TaskStatus},
    },
    model::{
        Message, MessageRole, ModelError, ModelErrorKind, ModelLimits, ModelRequest, ModelResponse,
        ToolCall, ToolDefinition,
    },
    runner::tools::{StructuredTools, ToolError, ToolErrorCode, ToolRequest, ToolResult},
};

const INSTRUCTIONS: &str = include_str!("prompts/worker.md");
/// Batas isi satu hasil tool yang dikembalikan ke model; selebihnya dipotong supaya konteks tidak meledak.
const MAX_TOOL_OUTPUT_BYTES: usize = 16 * 1024;
/// Total isi file allowed_paths yang disertakan di pesan pertama (selebihnya dibaca model lewat `read_file`).
const MAX_PRELOADED_BYTES: usize = 24 * 1024;

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
        let mut messages = vec![Message {
            role: MessageRole::System,
            content: INSTRUCTIONS.to_owned(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }];
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
                // Isi file literal di allowed_paths ikut dikirim di pesan pertama: satu giliran model (dan overhead
                // per-panggilan) lebih hemat daripada membiarkan model membacanya lewat tool.
                let request = ContextRequest {
                    excerpts: self
                        .context
                        .readable_allowed_files(self.contract, MAX_PRELOADED_BYTES),
                    search_terms: Vec::new(),
                };
                match self.context.build(self.contract, &request) {
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
            // Giliran assistant disimpan utuh, termasuk tool_calls: provider menolak pesan `tool` yang tidak didahului
            // pesan assistant yang memintanya.
            let text = response
                .content
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty());
            if text.is_some() || !response.tool_calls.is_empty() {
                messages.push(Message {
                    role: MessageRole::Assistant,
                    content: text.unwrap_or_default().to_owned(),
                    tool_call_id: None,
                    tool_calls: response.tool_calls.clone(),
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
                    Err(_) => {
                        // Argumen salah atau tool tidak dikenal adalah kesalahan model yang bisa diperbaiki: kembalikan
                        // sebagai hasil tool. Tetap dihitung ke batas panggilan supaya loop pasti berhenti.
                        usage.tool_calls = usage.tool_calls.saturating_add(1);
                        messages.push(tool_message(
                            &call.id,
                            format!(
                                "error: tool `{}` tidak dikenal atau argumennya tidak sesuai skema; periksa nama tool dan semua argumen wajib",
                                call.name
                            ),
                        ));
                        continue;
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
                            match checkpoint.outcome {
                                CheckpointOutcome::Succeeded => "panggilan ini sudah berhasil dijalankan sebelumnya; hasilnya tidak diputar ulang".to_owned(),
                                CheckpointOutcome::Failed => "panggilan ini sudah dijalankan sebelumnya dan gagal".to_owned(),
                                CheckpointOutcome::TimedOut => "panggilan ini sudah dijalankan sebelumnya dan melewati batas waktu".to_owned(),
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
                // Isi hasil tool dikembalikan ke model (terpotong); sebelumnya hanya flag sukses sehingga model tidak
                // pernah melihat isi file, diff, atau alasan kegagalan.
                messages.push(tool_message(
                    &call.id,
                    match &execution.result {
                        Ok(result) => render_result(result),
                        Err(error) => render_error(error),
                    },
                ));
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
                    // Hanya timeout yang fatal. Error lain (path ditolak, file tidak ada, patch gagal) dikembalikan ke model
                    // lewat pesan tool di atas supaya bisa diperbaiki; panggilan tool tetap dibatasi `max_tool_calls`.
                    Err(error_value) if error_value.code == ToolErrorCode::Timeout => {
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
    serde_json::from_str(ai_team::agent::json_text::extract(
        content.ok_or(WorkerError::InvalidModelResponse)?,
    ))
    .map_err(|_| WorkerError::InvalidModelResponse)
}

fn context_message(context: &BuiltContext) -> Message {
    Message {
        role: MessageRole::User,
        content: context
            .entries
            .iter()
            .map(|entry| match entry.source.kind {
                ContextSourceKind::TaskContract => entry.content.clone(),
                ContextSourceKind::SourceFile => format!(
                    "### Isi file `{}`{}\n{}",
                    entry.source.reference,
                    if entry.source.truncation_reason.is_some() {
                        " (TERPOTONG; baca sisanya dengan read_file)"
                    } else {
                        ""
                    },
                    entry.content
                ),
                ContextSourceKind::Artifact => {
                    format!(
                        "### Artifact `{}`\n{}",
                        entry.source.reference, entry.content
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        tool_call_id: None,
        tool_calls: Vec::new(),
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

/// Definisi tool lengkap dengan skema argumen. Tanpa skema, model nyata tidak tahu argumen apa yang harus diisi
/// dan memanggil tool dengan `{}`.
fn tool_definitions() -> Vec<ToolDefinition> {
    let object = |description: &str, properties: Value, required: &[&str]| {
        (
            description.to_owned(),
            json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
        )
    };
    let path = |what: &str| json!({"type":"string","description":what});
    [
        (
            "list_files",
            object(
                "Daftar file di bawah satu direktori (rekursif). Path harus berada dalam allowed_paths.",
                json!({"path": path("direktori relatif terhadap root repository")}),
                &["path"],
            ),
        ),
        (
            "search_code",
            object(
                "Cari teks literal pada file di bawah path. Mengembalikan baris `path:nomor: isi`.",
                json!({"path": path("direktori atau file relatif root repository"), "query": {"type":"string","description":"teks yang dicari (literal, tidak diawali '-')"}}),
                &["path", "query"],
            ),
        ),
        (
            "read_file",
            object(
                "Baca isi satu file teks. Path harus berada dalam allowed_paths.",
                json!({"path": path("file relatif terhadap root repository, mis. src/backend.js")}),
                &["path"],
            ),
        ),
        (
            "apply_patch",
            object(
                "Terapkan unified diff gaya git (`diff --git a/<path> b/<path>`, `---`, `+++`, hunk `@@`). Semua path harus dalam allowed_paths. Tanpa patch biner.",
                json!({"patch": {"type":"string","description":"teks diff utuh, diakhiri baris baru"}}),
                &["patch"],
            ),
        ),
        (
            "git_diff",
            object("Tampilkan perubahan yang sudah Anda buat di worktree.", json!({}), &[]),
        ),
        (
            "git_status",
            object("Tampilkan status file yang berubah di worktree.", json!({}), &[]),
        ),
        (
            "submit_artifact",
            object(
                "Simpan satu file sebagai artifact bukti kerja (jarang diperlukan).",
                json!({
                    "path": path("file relatif root repository"),
                    "artifact_id": {"type":"string","description":"UUID baru untuk artifact"},
                    "logical_name": {"type":"string","description":"nama logis artifact"},
                    "media_type": {"type":"string","description":"mis. text/plain"}
                }),
                &["path", "artifact_id", "logical_name", "media_type"],
            ),
        ),
        (
            "request_human",
            object(
                "Minta keputusan manusia bila tidak mungkin melanjutkan; mengakhiri pekerjaan Anda.",
                json!({"message": {"type":"string","description":"penjelasan singkat apa yang dibutuhkan"}}),
                &["message"],
            ),
        ),
    ]
    .into_iter()
    .map(|(name, (description, input_schema))| ToolDefinition {
        name: name.to_owned(),
        description,
        input_schema,
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

fn tool_message(call_id: &str, content: String) -> Message {
    Message {
        role: MessageRole::Tool,
        content: truncate_output(content),
        tool_call_id: Some(call_id.to_owned()),
        tool_calls: Vec::new(),
    }
}

/// Potong keluaran tool pada batas karakter yang valid, dengan penanda agar model tahu isinya tidak utuh.
fn truncate_output(mut text: String) -> String {
    if text.len() > MAX_TOOL_OUTPUT_BYTES {
        let mut end = MAX_TOOL_OUTPUT_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n[... keluaran dipotong ...]");
    }
    text
}

/// Teks hasil tool untuk model. Isi biner tidak ditampilkan mentah.
fn render_result(result: &ToolResult) -> String {
    let lossy = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    match result {
        ToolResult::Files(files) if files.is_empty() => "(tidak ada file)".to_owned(),
        ToolResult::Files(files) => files.join("\n"),
        ToolResult::SearchMatches(matches) if matches.is_empty() => {
            "(tidak ada kecocokan)".to_owned()
        }
        ToolResult::SearchMatches(matches) => matches
            .iter()
            .map(|found| format!("{}:{}: {}", found.path, found.line, found.text))
            .collect::<Vec<_>>()
            .join("\n"),
        ToolResult::File(bytes) => lossy(bytes),
        ToolResult::PatchApplied => "patch berhasil diterapkan".to_owned(),
        ToolResult::Diff(bytes) if bytes.is_empty() => "(belum ada perubahan)".to_owned(),
        ToolResult::Diff(bytes) => lossy(bytes),
        ToolResult::Status(status) if status.trim().is_empty() => "(bersih)".to_owned(),
        ToolResult::Status(status) => status.clone(),
        ToolResult::Artifact(metadata) => format!(
            "artifact tersimpan: id={} nama={} ukuran={}",
            metadata.artifact_id, metadata.logical_name, metadata.size
        ),
        ToolResult::HumanRequested { .. } => "permintaan bantuan manusia dicatat".to_owned(),
    }
}

fn render_error(error: &ToolError) -> String {
    format!("error ({:?}): {}", error.code, error.message)
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
