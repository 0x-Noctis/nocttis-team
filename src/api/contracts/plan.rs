use serde::{Deserialize, Serialize};

use crate::{
    api::contracts::task::TaskContractInput,
    domain::{
        project::{ApprovalDecision, PlanApproval, PlanStatus, ProposedPlan},
        task::{NonEmptyString, PositiveLimit, TaskContract, ValidationError},
    },
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedPlanInput {
    pub id: String,
    pub project_run_id: String,
    pub version: i64,
    pub tasks: Vec<TaskContractInput>,
    #[serde(default)]
    pub risk_flags: Vec<String>,
}

impl TryFrom<ProposedPlanInput> for ProposedPlan {
    type Error = ValidationError;

    fn try_from(input: ProposedPlanInput) -> Result<Self, Self::Error> {
        let plan = Self {
            id: NonEmptyString::parse("id", input.id)?,
            project_run_id: NonEmptyString::parse("project_run_id", input.project_run_id)?,
            version: PositiveLimit::new("version", input.version)?,
            tasks: input
                .tasks
                .into_iter()
                .map(TaskContract::try_from)
                .collect::<Result<_, _>>()?,
            risk_flags: input
                .risk_flags
                .into_iter()
                .map(|flag| NonEmptyString::parse("risk_flags", flag))
                .collect::<Result<_, _>>()?,
            status: PlanStatus::Proposed,
        };
        plan.validate()?;
        Ok(plan)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanApprovalInput {
    pub plan_id: String,
    pub actor_id: String,
    pub decision: ApprovalDecision,
    pub reason: Option<String>,
}

impl TryFrom<PlanApprovalInput> for PlanApproval {
    type Error = ValidationError;

    fn try_from(input: PlanApprovalInput) -> Result<Self, Self::Error> {
        let approval = Self {
            plan_id: NonEmptyString::parse("plan_id", input.plan_id)?,
            actor_id: NonEmptyString::parse("actor_id", input.actor_id)?,
            decision: input.decision,
            reason: input
                .reason
                .map(|reason| NonEmptyString::parse("reason", reason))
                .transpose()?,
        };
        approval.validate()?;
        Ok(approval)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ProposedPlanResponse {
    pub plan: ProposedPlan,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input() -> serde_json::Value {
        json!({
            "id": "plan", "project_run_id": "run", "version": 1,
            "tasks": [{
                "id": "task", "project_id": "project", "project_run_id": "run",
                "title": "Task", "role": "worker", "objective": "Ship",
                "allowed_paths": ["src/**"], "acceptance_criteria": ["Tests pass"],
                "verification_commands": ["cargo test"],
                "limits": {"max_input_tokens": 1, "max_output_tokens": 1, "max_tool_calls": 1, "max_attempts": 1, "timeout_seconds": 1}
            }],
            "risk_flags": ["Needs review"]
        })
    }

    #[test]
    fn plan_input_requires_ready_tasks_and_run_match() {
        let plan =
            ProposedPlan::try_from(serde_json::from_value::<ProposedPlanInput>(input()).unwrap())
                .unwrap();
        assert_eq!(plan.status, PlanStatus::Proposed);
        for (path, bad, field) in [
            ("version", json!(0), "version"),
            ("tasks", json!([]), "tasks"),
            ("risk_flags", json!([" "]), "risk_flags"),
        ] {
            let mut value = input();
            value[path] = bad;
            assert_eq!(
                ProposedPlan::try_from(serde_json::from_value::<ProposedPlanInput>(value).unwrap())
                    .unwrap_err()
                    .field,
                field
            );
        }
        let mut value = input();
        value["tasks"][0]["project_run_id"] = json!("other");
        assert_eq!(
            ProposedPlan::try_from(serde_json::from_value::<ProposedPlanInput>(value).unwrap())
                .unwrap_err()
                .field,
            "tasks"
        );
        let mut value = input();
        value["tasks"][0]["acceptance_criteria"] = json!([]);
        assert_eq!(
            ProposedPlan::try_from(serde_json::from_value::<ProposedPlanInput>(value).unwrap())
                .unwrap_err()
                .field,
            "acceptance_criteria"
        );
        for dependencies in [json!(["missing"]), json!(["task"])] {
            let mut value = input();
            value["tasks"][0]["depends_on"] = dependencies;
            assert_eq!(
                ProposedPlan::try_from(serde_json::from_value::<ProposedPlanInput>(value).unwrap())
                    .unwrap_err()
                    .field,
                "tasks"
            );
        }
        let mut value = input();
        value["status"] = json!("APPROVED");
        assert!(serde_json::from_value::<ProposedPlanInput>(value).is_err());
    }

    #[test]
    fn approval_input_requires_explicit_actor_and_rejection_reason() {
        let input =
            json!({"plan_id": "plan", "actor_id": "human", "decision": "APPROVED", "reason": null});
        assert!(
            PlanApproval::try_from(
                serde_json::from_value::<PlanApprovalInput>(input.clone()).unwrap()
            )
            .is_ok()
        );
        for field in ["plan_id", "actor_id"] {
            let mut value = input.clone();
            value[field] = json!(" ");
            assert_eq!(
                PlanApproval::try_from(serde_json::from_value::<PlanApprovalInput>(value).unwrap())
                    .unwrap_err()
                    .field,
                field
            );
        }
        let mut value = input.clone();
        value["decision"] = json!("REJECTED");
        assert_eq!(
            PlanApproval::try_from(serde_json::from_value::<PlanApprovalInput>(value).unwrap())
                .unwrap_err()
                .field,
            "reason"
        );
        let mut value = input;
        value["decision"] = json!("AUTOMATIC");
        assert!(serde_json::from_value::<PlanApprovalInput>(value).is_err());
    }
}
