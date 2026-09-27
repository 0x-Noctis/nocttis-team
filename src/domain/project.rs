use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::domain::{
    dag::{Dag, TaskNode},
    task::{NonEmptyString, PositiveLimit, TaskContract, TaskStatus, ValidationError},
};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepositoryPath(String);

impl RepositoryPath {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, ValidationError> {
        let path = std::fs::canonicalize(value.as_ref()).map_err(|_| ValidationError {
            field: "repository_path",
            message: "must be an existing directory",
        })?;
        if !path.is_dir() {
            return Err(ValidationError {
                field: "repository_path",
                message: "must be an existing directory",
            });
        }
        if !std::process::Command::new("git")
            .args([
                "-C",
                path.to_str().unwrap_or(""),
                "rev-parse",
                "--show-toplevel",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
        {
            return Err(ValidationError {
                field: "repository_path",
                message: "must be a Git repository",
            });
        }
        let path = path.to_str().ok_or(ValidationError {
            field: "repository_path",
            message: "must be a UTF-8 directory path",
        })?;
        Ok(Self(path.to_owned()))
    }

    pub fn as_path(&self) -> &std::path::Path {
        std::path::Path::new(&self.0)
    }
}

impl TryFrom<String> for RepositoryPath {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<RepositoryPath> for String {
    fn from(value: RepositoryPath) -> Self {
        value.0
    }
}

impl FromStr for RepositoryPath {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub id: NonEmptyString,
    pub name: NonEmptyString,
    pub repository_path: RepositoryPath,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RunStatus {
    Planning,
    AwaitingApproval,
    Running,
    Paused,
    Done,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectRun {
    pub id: NonEmptyString,
    pub project_id: NonEmptyString,
    pub objective: NonEmptyString,
    pub acceptance_criteria: Vec<NonEmptyString>,
    pub token_budget: PositiveLimit,
    pub status: RunStatus,
}

impl ProjectRun {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.acceptance_criteria.is_empty() {
            return Err(ValidationError {
                field: "acceptance_criteria",
                message: "must not be empty",
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PlanStatus {
    Proposed,
    Approved,
    Rejected,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedPlan {
    pub id: NonEmptyString,
    pub project_run_id: NonEmptyString,
    pub version: PositiveLimit,
    pub tasks: Vec<TaskContract>,
    pub risk_flags: Vec<NonEmptyString>,
    pub status: PlanStatus,
}

impl ProposedPlan {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.tasks.is_empty() {
            return Err(ValidationError {
                field: "tasks",
                message: "must not be empty",
            });
        }
        for task in &self.tasks {
            if task.project_run_id != self.project_run_id {
                return Err(ValidationError {
                    field: "tasks",
                    message: "must belong to this run",
                });
            }
            task.validate_for_status(crate::domain::task::TaskStatus::Ready)?;
        }
        let nodes: Vec<_> = self
            .tasks
            .iter()
            .map(|contract| TaskNode {
                contract,
                status: TaskStatus::Planned,
            })
            .collect();
        Dag::new(&nodes).map_err(|_| ValidationError {
            field: "tasks",
            message: "must form a valid dependency graph",
        })?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ApprovalDecision {
    Approved,
    Rejected,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanApproval {
    pub plan_id: NonEmptyString,
    pub actor_id: NonEmptyString,
    pub decision: ApprovalDecision,
    pub reason: Option<NonEmptyString>,
}

impl PlanApproval {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.decision == ApprovalDecision::Rejected && self.reason.is_none() {
            return Err(ValidationError {
                field: "reason",
                message: "required when rejecting a plan",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_path_is_canonical_and_directory_only() {
        let current = std::env::current_dir().unwrap();
        let path = RepositoryPath::parse(".").unwrap();
        assert_eq!(path.as_path(), std::fs::canonicalize(current).unwrap());
        for bad in ["", "does-not-exist", "Cargo.toml"] {
            assert_eq!(
                RepositoryPath::parse(bad).unwrap_err().field,
                "repository_path"
            );
        }
    }

    #[test]
    fn run_requires_criteria_and_positive_budget() {
        let mut run = ProjectRun {
            id: NonEmptyString::parse("id", "run").unwrap(),
            project_id: NonEmptyString::parse("project_id", "project").unwrap(),
            objective: NonEmptyString::parse("objective", "Ship feature").unwrap(),
            acceptance_criteria: vec![],
            token_budget: PositiveLimit::new("token_budget", 1).unwrap(),
            status: RunStatus::Planning,
        };
        assert_eq!(run.validate().unwrap_err().field, "acceptance_criteria");
        run.acceptance_criteria
            .push(NonEmptyString::parse("acceptance_criteria", "Tests pass").unwrap());
        assert!(run.validate().is_ok());
        assert_eq!(
            PositiveLimit::new("token_budget", 0).unwrap_err().field,
            "token_budget"
        );
    }

    #[test]
    fn approval_requires_actor_and_rejection_reason() {
        let approval = PlanApproval {
            plan_id: NonEmptyString::parse("plan_id", "plan").unwrap(),
            actor_id: NonEmptyString::parse("actor_id", "human").unwrap(),
            decision: ApprovalDecision::Rejected,
            reason: None,
        };
        assert_eq!(approval.validate().unwrap_err().field, "reason");
        assert!(serde_json::from_str::<ApprovalDecision>("\"AUTOMATIC\"").is_err());
    }
}
