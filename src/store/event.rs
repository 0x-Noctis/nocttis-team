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
    pub attempt: i64,
    pub provider_id: NonEmptyString,
    pub model_id: NonEmptyString,
    pub status: NonEmptyString,
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
