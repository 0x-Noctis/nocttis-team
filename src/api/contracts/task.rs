use serde::{Deserialize, Serialize};

use crate::domain::task::{
    AllowedPath, MaxAttempts, NonEmptyString, PositiveLimit, TaskContract, TaskLimits,
    ValidationError,
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskLimitsInput {
    pub max_input_tokens: i64,
    pub max_output_tokens: i64,
    pub max_tool_calls: i64,
    pub max_attempts: i64,
    pub timeout_seconds: i64,
}

impl TryFrom<TaskLimitsInput> for TaskLimits {
    type Error = ValidationError;

    fn try_from(input: TaskLimitsInput) -> Result<Self, Self::Error> {
        Ok(Self {
            max_input_tokens: PositiveLimit::new("max_input_tokens", input.max_input_tokens)?,
            max_output_tokens: PositiveLimit::new("max_output_tokens", input.max_output_tokens)?,
            max_tool_calls: PositiveLimit::new("max_tool_calls", input.max_tool_calls)?,
            max_attempts: MaxAttempts::new(input.max_attempts)?,
            timeout_seconds: PositiveLimit::new("timeout_seconds", input.timeout_seconds)?,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskContractInput {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub role: String,
    pub objective: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    #[serde(default)]
    pub context_refs: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub verification_commands: Vec<String>,
    pub limits: TaskLimitsInput,
}

impl TryFrom<TaskContractInput> for TaskContract {
    type Error = ValidationError;

    fn try_from(input: TaskContractInput) -> Result<Self, Self::Error> {
        Ok(Self {
            id: NonEmptyString::parse("id", input.id)?,
            project_id: NonEmptyString::parse("project_id", input.project_id)?,
            title: NonEmptyString::parse("title", input.title)?,
            role: NonEmptyString::parse("role", input.role)?,
            objective: NonEmptyString::parse("objective", input.objective)?,
            depends_on: non_empty_list("depends_on", input.depends_on)?,
            allowed_paths: input
                .allowed_paths
                .into_iter()
                .map(AllowedPath::parse)
                .collect::<Result<_, _>>()?,
            context_refs: non_empty_list("context_refs", input.context_refs)?,
            acceptance_criteria: non_empty_list(
                "acceptance_criteria",
                input.acceptance_criteria,
            )?,
            verification_commands: non_empty_list(
                "verification_commands",
                input.verification_commands,
            )?,
            limits: input.limits.try_into()?,
        })
    }
}

fn non_empty_list(
    field: &'static str,
    values: Vec<String>,
) -> Result<Vec<NonEmptyString>, ValidationError> {
    values
        .into_iter()
        .map(|value| NonEmptyString::parse(field, value))
        .collect()
}

#[derive(Clone, Debug, Serialize)]
pub struct TaskContractResponse {
    pub contract: TaskContract,
}

impl From<TaskContract> for TaskContractResponse {
    fn from(contract: TaskContract) -> Self {
        Self { contract }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::task::MAX_SAFE_INTEGER;
    use serde_json::json;

    fn input() -> serde_json::Value {
        json!({
            "id": "BE-014",
            "project_id": "P-001",
            "title": "Create endpoint",
            "role": "backend_engineer",
            "objective": "Create endpoint safely",
            "depends_on": [],
            "allowed_paths": ["src/products/**"],
            "context_refs": [],
            "acceptance_criteria": ["Tests pass"],
            "verification_commands": ["cargo test products"],
            "limits": {
                "max_input_tokens": 30000,
                "max_output_tokens": 8000,
                "max_tool_calls": 40,
                "max_attempts": 2,
                "timeout_seconds": 1200
            }
        })
    }

    #[test]
    fn dto_validates_complete_contract() {
        let dto: TaskContractInput = serde_json::from_value(input()).unwrap();
        let contract = TaskContract::try_from(dto).unwrap();
        assert_eq!(contract.id.as_str(), "BE-014");
        assert_eq!(contract.allowed_paths[0].as_str(), "src/products/**");
    }

    #[test]
    fn dto_rejects_unknown_fields_at_both_levels() {
        let mut top = input();
        top["secret"] = json!("must-not-enter-domain");
        assert!(serde_json::from_value::<TaskContractInput>(top).is_err());

        let mut limits = input();
        limits["limits"]["unexpected"] = json!(1);
        assert!(serde_json::from_value::<TaskContractInput>(limits).is_err());
    }

    #[test]
    fn dto_rejects_empty_scalars_paths_and_list_items() {
        for field in ["id", "project_id", "title", "role", "objective"] {
            let mut value = input();
            value[field] = json!(" ");
            let dto: TaskContractInput = serde_json::from_value(value).unwrap();
            assert_eq!(TaskContract::try_from(dto).unwrap_err().field, field);
        }

        for (field, bad) in [
            ("allowed_paths", json!(["../secret"])),
            ("depends_on", json!([""])),
            ("context_refs", json!([" "])),
            ("acceptance_criteria", json!([""])),
            ("verification_commands", json!([" "])),
        ] {
            let mut value = input();
            value[field] = bad;
            let dto: TaskContractInput = serde_json::from_value(value).unwrap();
            assert_eq!(TaskContract::try_from(dto).unwrap_err().field, field);
        }
    }

    #[test]
    fn dto_rejects_invalid_limits() {
        for (field, values) in [
            ("max_input_tokens", vec![0, -1, MAX_SAFE_INTEGER + 1]),
            ("max_output_tokens", vec![0, -1, MAX_SAFE_INTEGER + 1]),
            ("max_tool_calls", vec![0, -1, MAX_SAFE_INTEGER + 1]),
            ("timeout_seconds", vec![0, -1, MAX_SAFE_INTEGER + 1]),
            ("max_attempts", vec![0, -1, 11]),
        ] {
            for invalid in values {
                let mut value = input();
                value["limits"][field] = json!(invalid);
                let dto: TaskContractInput = serde_json::from_value(value).unwrap();
                assert_eq!(TaskContract::try_from(dto).unwrap_err().field, field);
            }
        }
    }
}
