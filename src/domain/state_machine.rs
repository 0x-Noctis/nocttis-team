use std::fmt;

use serde::{Deserialize, Serialize};

use super::task::{TaskContract, TaskStatus, ValidationError};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    Worker,
    Reviewer,
    Verifier,
    Integrator,
    Human,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransitionError {
    InvalidTransition {
        from: TaskStatus,
        to: TaskStatus,
        actor: Actor,
    },
    InvalidContract(ValidationError),
}

impl fmt::Display for TransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTransition { from, to, actor } => {
                write!(formatter, "{actor:?} cannot transition {from:?} to {to:?}")
            }
            Self::InvalidContract(error) => write!(formatter, "invalid task contract: {error}"),
        }
    }
}

impl std::error::Error for TransitionError {}

pub const VALID_TRANSITIONS: &[(TaskStatus, TaskStatus, Actor)] = &[
    (TaskStatus::Draft, TaskStatus::Planned, Actor::System),
    (TaskStatus::Planned, TaskStatus::Ready, Actor::System),
    (TaskStatus::Ready, TaskStatus::Assigned, Actor::System),
    (TaskStatus::Assigned, TaskStatus::Running, Actor::System),
    (TaskStatus::Running, TaskStatus::SelfCheck, Actor::Worker),
    (TaskStatus::SelfCheck, TaskStatus::Review, Actor::Worker),
    (
        TaskStatus::Review,
        TaskStatus::ChangesRequested,
        Actor::Reviewer,
    ),
    (TaskStatus::Review, TaskStatus::Verify, Actor::Reviewer),
    (
        TaskStatus::ChangesRequested,
        TaskStatus::Ready,
        Actor::System,
    ),
    (TaskStatus::Verify, TaskStatus::Failed, Actor::Verifier),
    (TaskStatus::Verify, TaskStatus::Integrate, Actor::Verifier),
    (TaskStatus::Failed, TaskStatus::Ready, Actor::System),
    (TaskStatus::Integrate, TaskStatus::Done, Actor::Integrator),
    (
        TaskStatus::Integrate,
        TaskStatus::Conflict,
        Actor::Integrator,
    ),
    (TaskStatus::Integrate, TaskStatus::Ready, Actor::Integrator),
    (
        TaskStatus::Conflict,
        TaskStatus::NeedsHuman,
        Actor::Integrator,
    ),
    // Penolakan reviewer pada attempt terakhir: tidak ada percobaan ulang, jadi manusia yang memutuskan.
    (
        TaskStatus::ChangesRequested,
        TaskStatus::NeedsHuman,
        Actor::System,
    ),
    (TaskStatus::NeedsHuman, TaskStatus::Ready, Actor::Human),
];

pub fn recovery_transition(
    contract: &TaskContract,
    from: TaskStatus,
    ambiguous_side_effect: bool,
) -> Result<TaskStatus, TransitionError> {
    let to = if ambiguous_side_effect {
        TaskStatus::NeedsHuman
    } else {
        TaskStatus::Ready
    };
    let allowed = matches!(
        from,
        TaskStatus::Running
            | TaskStatus::SelfCheck
            | TaskStatus::Review
            | TaskStatus::Verify
            | TaskStatus::Integrate
    ) || (from == TaskStatus::Assigned && !ambiguous_side_effect);
    if !allowed {
        return Err(TransitionError::InvalidTransition {
            from,
            to,
            actor: Actor::System,
        });
    }
    contract
        .validate_for_status(to)
        .map_err(TransitionError::InvalidContract)?;
    Ok(to)
}

pub fn transition(
    contract: &TaskContract,
    from: TaskStatus,
    to: TaskStatus,
    actor: Actor,
) -> Result<TaskStatus, TransitionError> {
    let cancelled = to == TaskStatus::Cancelled
        && !from.is_terminal()
        && matches!(actor, Actor::Human | Actor::System);
    if !cancelled && !VALID_TRANSITIONS.contains(&(from, to, actor)) {
        return Err(TransitionError::InvalidTransition { from, to, actor });
    }
    contract
        .validate_for_status(to)
        .map_err(TransitionError::InvalidContract)?;
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::task::{
        AllowedPath, MaxAttempts, NonEmptyString, PositiveLimit, TaskLimits,
    };

    const STATUSES: [TaskStatus; 15] = [
        TaskStatus::Draft,
        TaskStatus::Planned,
        TaskStatus::Ready,
        TaskStatus::Assigned,
        TaskStatus::Running,
        TaskStatus::SelfCheck,
        TaskStatus::Review,
        TaskStatus::ChangesRequested,
        TaskStatus::Verify,
        TaskStatus::Failed,
        TaskStatus::Integrate,
        TaskStatus::Conflict,
        TaskStatus::NeedsHuman,
        TaskStatus::Done,
        TaskStatus::Cancelled,
    ];
    const ACTORS: [Actor; 6] = [
        Actor::Worker,
        Actor::Reviewer,
        Actor::Verifier,
        Actor::Integrator,
        Actor::Human,
        Actor::System,
    ];

    fn text(field: &'static str, value: &str) -> NonEmptyString {
        NonEmptyString::parse(field, value).unwrap()
    }

    fn contract() -> TaskContract {
        TaskContract {
            id: text("id", "BE-014"),
            project_id: text("project_id", "P-001"),
            project_run_id: text("project_run_id", "RUN-001"),
            title: text("title", "Create endpoint"),
            role: text("role", "backend_engineer"),
            objective: text("objective", "Create endpoint safely"),
            depends_on: Vec::new(),
            allowed_paths: vec![AllowedPath::parse("src/products/**").unwrap()],
            context_refs: Vec::new(),
            acceptance_criteria: vec![text("acceptance_criteria", "Tests pass")],
            verification_commands: vec![text("verification_commands", "cargo test")],
            limits: TaskLimits {
                max_input_tokens: PositiveLimit::new("max_input_tokens", 1).unwrap(),
                max_output_tokens: PositiveLimit::new("max_output_tokens", 1).unwrap(),
                max_tool_calls: PositiveLimit::new("max_tool_calls", 1).unwrap(),
                max_attempts: MaxAttempts::new(1).unwrap(),
                timeout_seconds: PositiveLimit::new("timeout_seconds", 1).unwrap(),
            },
        }
    }

    #[test]
    fn recovery_is_separate_from_normal_workflow() {
        let task = contract();
        for from in STATUSES {
            for ambiguous in [false, true] {
                let expected = if ambiguous {
                    TaskStatus::NeedsHuman
                } else {
                    TaskStatus::Ready
                };
                let allowed = matches!(
                    from,
                    TaskStatus::Running
                        | TaskStatus::SelfCheck
                        | TaskStatus::Review
                        | TaskStatus::Verify
                        | TaskStatus::Integrate
                ) || (from == TaskStatus::Assigned && !ambiguous);
                let result = recovery_transition(&task, from, ambiguous);
                if allowed {
                    assert_eq!(result, Ok(expected));
                    assert!(transition(&task, from, expected, Actor::System).is_err());
                } else {
                    assert!(result.is_err());
                }
            }
        }
    }

    #[test]
    fn transition_table_accepts_every_declared_transition() {
        let task = contract();
        for &(from, to, actor) in VALID_TRANSITIONS {
            assert_eq!(transition(&task, from, to, actor), Ok(to));
        }
        for from in STATUSES {
            if !from.is_terminal() {
                for actor in [Actor::Human, Actor::System] {
                    assert_eq!(
                        transition(&task, from, TaskStatus::Cancelled, actor),
                        Ok(TaskStatus::Cancelled)
                    );
                }
            }
        }
    }

    #[test]
    fn transition_table_rejects_every_undeclared_transition() {
        let task = contract();
        for from in STATUSES {
            for to in STATUSES {
                for actor in ACTORS {
                    let declared = VALID_TRANSITIONS.contains(&(from, to, actor));
                    let cancellation = to == TaskStatus::Cancelled
                        && !from.is_terminal()
                        && matches!(actor, Actor::Human | Actor::System);
                    if !declared && !cancellation {
                        assert!(
                            matches!(
                                transition(&task, from, to, actor),
                                Err(TransitionError::InvalidTransition { .. })
                            ),
                            "accepted {actor:?}: {from:?} to {to:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn worker_cannot_set_done() {
        let task = contract();
        for from in STATUSES {
            assert!(matches!(
                transition(&task, from, TaskStatus::Done, Actor::Worker),
                Err(TransitionError::InvalidTransition { .. })
            ));
        }
    }

    #[test]
    fn transition_to_ready_validates_contract() {
        let mut task = contract();
        task.acceptance_criteria.clear();
        assert!(matches!(
            transition(&task, TaskStatus::Planned, TaskStatus::Ready, Actor::System),
            Err(TransitionError::InvalidContract(_))
        ));
    }
}
