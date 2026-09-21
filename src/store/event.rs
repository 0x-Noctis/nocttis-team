use serde_json::Value;
use uuid::Uuid;

use crate::domain::{
    state_machine::Actor,
    task::{MAX_SAFE_INTEGER, NonEmptyString, TaskStatus},
};

#[derive(Clone, Debug, PartialEq)]
pub struct TaskEvent {
    pub id: i64,
    pub task_id: NonEmptyString,
    pub actor: Actor,
    pub event_type: NonEmptyString,
    pub from_status: Option<TaskStatus>,
    pub to_status: Option<TaskStatus>,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentAttempt {
    pub id: Uuid,
    pub task_id: NonEmptyString,
    pub role: NonEmptyString,
    pub attempt: i64,
    pub provider_id: NonEmptyString,
    pub model_id: NonEmptyString,
    pub status: NonEmptyString,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptStatus {
    Assigned,
    Running,
    RecoveryRequired,
    Completed,
    Failed,
}

impl AttemptStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Assigned => "assigned",
            Self::Running => "running",
            Self::RecoveryRequired => "recovery_required",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "assigned" => Some(Self::Assigned),
            "running" => Some(Self::Running),
            "recovery_required" => Some(Self::RecoveryRequired),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeAttempt {
    pub id: Uuid,
    pub task_id: NonEmptyString,
    pub role: NonEmptyString,
    pub provider_id: NonEmptyString,
    pub model_id: NonEmptyString,
    pub attempt: i64,
    pub status: AttemptStatus,
    pub branch: String,
    pub base_commit: String,
    pub heartbeat_unix_ms: Option<i64>,
    pub finished_unix_ms: Option<i64>,
    pub retain_until_unix_ms: Option<i64>,
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimAttempt {
    pub id: Uuid,
    pub task_id: NonEmptyString,
    pub role: NonEmptyString,
    pub provider_id: NonEmptyString,
    pub model_id: NonEmptyString,
    pub branch: String,
    pub base_commit: String,
    pub retention_seconds: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchClaim {
    pub id: Uuid,
    pub role: NonEmptyString,
    pub provider_id: NonEmptyString,
    pub model_id: NonEmptyString,
    pub branch: String,
    pub base_commit: String,
    pub retention_seconds: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryDisposition {
    Requeued,
    RecoveryRequired,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryResult {
    pub attempt_id: Uuid,
    pub task_id: NonEmptyString,
    pub disposition: RecoveryDisposition,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetentionClaim {
    pub attempt: RuntimeAttempt,
    pub owner_token: Uuid,
    pub lease_until_unix_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttemptUpdate {
    pub status: AttemptStatus,
    pub error_code: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolOutcome {
    Succeeded,
    Failed,
    TimedOut,
}

impl ToolOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "timed_out" => Some(Self::TimedOut),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallMetadata {
    pub outcome: ToolOutcome,
    pub duration_ms: i64,
    pub artifact_id: Option<Uuid>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolCallReservation {
    New,
    InProgress,
    Completed(ToolCallMetadata),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactRecord {
    pub id: Uuid,
    pub task_id: NonEmptyString,
    pub kind: NonEmptyString,
    pub logical_name: NonEmptyString,
    pub media_type: NonEmptyString,
    pub size: i64,
    pub checksum: String,
}

impl AgentAttempt {
    pub fn validate(&self) -> Result<(), NumericError> {
        positive("attempt", self.attempt)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: i64,
    pub cached_tokens: i64,
    pub output_tokens: i64,
    pub tool_calls: i64,
    pub latency_ms: i64,
    pub estimated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegrationStatus {
    Prepared,
    Applied,
    Completed,
    Conflict,
}

impl IntegrationStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Applied => "applied",
            Self::Completed => "completed",
            Self::Conflict => "conflict",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "prepared" => Some(Self::Prepared),
            "applied" => Some(Self::Applied),
            "completed" => Some(Self::Completed),
            "conflict" => Some(Self::Conflict),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrationOperation {
    pub id: Uuid,
    pub attempt_id: Uuid,
    pub task_id: NonEmptyString,
    pub source_branch: String,
    pub source_base_commit: String,
    pub target_id: String,
    pub target_branch: String,
    pub target_base_commit: String,
    pub patch_sha256: String,
    pub owner_token: Uuid,
    pub status: IntegrationStatus,
}

impl Usage {
    pub fn validate(&self) -> Result<(), NumericError> {
        for (field, value) in [
            ("input_tokens", self.input_tokens),
            ("cached_tokens", self.cached_tokens),
            ("output_tokens", self.output_tokens),
            ("tool_calls", self.tool_calls),
            ("latency_ms", self.latency_ms),
        ] {
            non_negative(field, value)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NumericError {
    pub field: &'static str,
}

fn positive(field: &'static str, value: i64) -> Result<(), NumericError> {
    if !(1..=MAX_SAFE_INTEGER).contains(&value) {
        return Err(NumericError { field });
    }
    Ok(())
}

fn non_negative(field: &'static str, value: i64) -> Result<(), NumericError> {
    if !(0..=MAX_SAFE_INTEGER).contains(&value) {
        return Err(NumericError { field });
    }
    Ok(())
}
