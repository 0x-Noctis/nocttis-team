use serde::{Deserialize, Serialize};

use crate::domain::{
    project::{Project, ProjectRun, RepositoryPath, RunStatus},
    task::{NonEmptyString, PositiveLimit, ValidationError},
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectInput {
    pub id: String,
    pub name: String,
    pub repository_path: String,
}

impl TryFrom<ProjectInput> for Project {
    type Error = ValidationError;

    fn try_from(input: ProjectInput) -> Result<Self, Self::Error> {
        Ok(Self {
            id: NonEmptyString::parse("id", input.id)?,
            name: NonEmptyString::parse("name", input.name)?,
            repository_path: RepositoryPath::parse(input.repository_path)?,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectRunInput {
    pub id: String,
    pub project_id: String,
    pub objective: String,
    pub acceptance_criteria: Vec<String>,
    pub token_budget: i64,
}

impl TryFrom<ProjectRunInput> for ProjectRun {
    type Error = ValidationError;

    fn try_from(input: ProjectRunInput) -> Result<Self, Self::Error> {
        let run = Self {
            id: NonEmptyString::parse("id", input.id)?,
            project_id: NonEmptyString::parse("project_id", input.project_id)?,
            objective: NonEmptyString::parse("objective", input.objective)?,
            acceptance_criteria: input
                .acceptance_criteria
                .into_iter()
                .map(|value| NonEmptyString::parse("acceptance_criteria", value))
                .collect::<Result<_, _>>()?,
            token_budget: PositiveLimit::new("token_budget", input.token_budget)?,
            status: RunStatus::Planning,
        };
        run.validate()?;
        Ok(run)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ProjectResponse {
    pub project: Project,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProjectRunResponse {
    pub run: ProjectRun,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::task::MAX_SAFE_INTEGER;
    use serde_json::json;

    #[test]
    fn project_input_canonicalizes_and_rejects_bad_paths() {
        let input = json!({"id": "p", "name": "Project", "repository_path": "."});
        let project =
            Project::try_from(serde_json::from_value::<ProjectInput>(input.clone()).unwrap())
                .unwrap();
        assert_eq!(
            project.repository_path.as_path(),
            std::fs::canonicalize(".").unwrap()
        );
        for bad in ["", "missing-project-repository", "Cargo.toml"] {
            let mut value = input.clone();
            value["repository_path"] = json!(bad);
            assert_eq!(
                Project::try_from(serde_json::from_value::<ProjectInput>(value).unwrap())
                    .unwrap_err()
                    .field,
                "repository_path"
            );
        }
        for field in ["id", "name"] {
            let mut value = input.clone();
            value[field] = json!(" ");
            assert_eq!(
                Project::try_from(serde_json::from_value::<ProjectInput>(value).unwrap())
                    .unwrap_err()
                    .field,
                field
            );
        }
        let mut value = input;
        value["unknown"] = json!(true);
        assert!(serde_json::from_value::<ProjectInput>(value).is_err());
    }

    #[test]
    fn run_input_validates_boundaries() {
        let input = json!({"id": "run", "project_id": "p", "objective": "Ship", "acceptance_criteria": ["Tests pass"], "token_budget": 100});
        let run =
            ProjectRun::try_from(serde_json::from_value::<ProjectRunInput>(input.clone()).unwrap())
                .unwrap();
        assert_eq!(run.status, RunStatus::Planning);
        for field in ["id", "project_id", "objective"] {
            let mut value = input.clone();
            value[field] = json!(" ");
            assert_eq!(
                ProjectRun::try_from(serde_json::from_value::<ProjectRunInput>(value).unwrap())
                    .unwrap_err()
                    .field,
                field
            );
        }
        for criteria in [json!([]), json!([" "])] {
            let mut value = input.clone();
            value["acceptance_criteria"] = criteria;
            assert_eq!(
                ProjectRun::try_from(serde_json::from_value::<ProjectRunInput>(value).unwrap())
                    .unwrap_err()
                    .field,
                "acceptance_criteria"
            );
        }
        for budget in [0, -1, MAX_SAFE_INTEGER + 1] {
            let mut value = input.clone();
            value["token_budget"] = json!(budget);
            assert_eq!(
                ProjectRun::try_from(serde_json::from_value::<ProjectRunInput>(value).unwrap())
                    .unwrap_err()
                    .field,
                "token_budget"
            );
        }
        let mut value = input;
        value["status"] = json!("RUNNING");
        assert!(serde_json::from_value::<ProjectRunInput>(value).is_err());
    }
}
