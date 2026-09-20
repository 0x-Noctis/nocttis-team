use std::{fmt, path::Component};

use serde::{Deserialize, Serialize};

pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationError {
    pub field: &'static str,
    pub message: &'static str,
}

impl ValidationError {
    const fn new(field: &'static str, message: &'static str) -> Self {
        Self { field, message }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.field, self.message)
    }
}

impl std::error::Error for ValidationError {}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct NonEmptyString(String);

impl NonEmptyString {
    pub fn parse(field: &'static str, value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ValidationError::new(field, "must not be empty"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for NonEmptyString {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse("value", value)
    }
}

impl From<NonEmptyString> for String {
    fn from(value: NonEmptyString) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct AllowedPath(String);

impl AllowedPath {
    pub fn parse(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        let normalized = value.replace('\\', "/");
        let windows_absolute = normalized.starts_with("//")
            || normalized
                .as_bytes()
                .get(1)
                .is_some_and(|separator| *separator == b':');
        let invalid_component = std::path::Path::new(&normalized)
            .components()
            .any(|component| {
                matches!(
                    component,
                    Component::CurDir | Component::ParentDir | Component::RootDir
                )
            });
        if value.trim().is_empty() {
            return Err(ValidationError::new(
                "allowed_paths",
                "must not contain empty paths",
            ));
        }
        if windows_absolute || invalid_component || value.chars().any(char::is_control) {
            return Err(ValidationError::new(
                "allowed_paths",
                "must contain only relative paths without traversal",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AllowedPath {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<AllowedPath> for String {
    fn from(value: AllowedPath) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct PositiveLimit(i64);

impl PositiveLimit {
    pub fn new(field: &'static str, value: i64) -> Result<Self, ValidationError> {
        if !(1..=MAX_SAFE_INTEGER).contains(&value) {
            return Err(ValidationError::new(
                field,
                "must be positive and no greater than Number.MAX_SAFE_INTEGER",
            ));
        }
        Ok(Self(value))
    }

    pub fn get(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for PositiveLimit {
    type Error = ValidationError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new("value", value)
    }
}

impl From<PositiveLimit> for i64 {
    fn from(value: PositiveLimit) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct MaxAttempts(i64);

impl MaxAttempts {
    pub fn new(value: i64) -> Result<Self, ValidationError> {
        if !(1..=10).contains(&value) {
            return Err(ValidationError::new(
                "max_attempts",
                "must be between 1 and 10",
            ));
        }
        Ok(Self(value))
    }

    pub fn get(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for MaxAttempts {
    type Error = ValidationError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<MaxAttempts> for i64 {
    fn from(value: MaxAttempts) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskLimits {
    pub max_input_tokens: PositiveLimit,
    pub max_output_tokens: PositiveLimit,
    pub max_tool_calls: PositiveLimit,
    pub max_attempts: MaxAttempts,
    pub timeout_seconds: PositiveLimit,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskStatus {
    Draft,
    Planned,
    Ready,
    Assigned,
    Running,
    SelfCheck,
    Review,
    ChangesRequested,
    Verify,
    Failed,
    Integrate,
    Conflict,
    NeedsHuman,
    Done,
    Cancelled,
}

impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskContract {
    pub id: NonEmptyString,
    pub project_id: NonEmptyString,
    pub title: NonEmptyString,
    pub role: NonEmptyString,
    pub objective: NonEmptyString,
    pub depends_on: Vec<NonEmptyString>,
    pub allowed_paths: Vec<AllowedPath>,
    pub context_refs: Vec<NonEmptyString>,
    pub acceptance_criteria: Vec<NonEmptyString>,
    pub verification_commands: Vec<NonEmptyString>,
    pub limits: TaskLimits,
}

impl TaskContract {
    pub fn validate_for_status(&self, status: TaskStatus) -> Result<(), ValidationError> {
        if status == TaskStatus::Ready {
            for (field, empty) in [
                ("acceptance_criteria", self.acceptance_criteria.is_empty()),
                ("allowed_paths", self.allowed_paths.is_empty()),
                (
                    "verification_commands",
                    self.verification_commands.is_empty(),
                ),
            ] {
                if empty {
                    return Err(ValidationError::new(
                        field,
                        "must not be empty before READY",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract() -> TaskContract {
        TaskContract {
            id: NonEmptyString::parse("id", "BE-014").unwrap(),
            project_id: NonEmptyString::parse("project_id", "P-001").unwrap(),
            title: NonEmptyString::parse("title", "Create product endpoint").unwrap(),
            role: NonEmptyString::parse("role", "backend_engineer").unwrap(),
            objective: NonEmptyString::parse("objective", "Create products safely").unwrap(),
            depends_on: Vec::new(),
            allowed_paths: vec![AllowedPath::parse("src/products/**").unwrap()],
            context_refs: Vec::new(),
            acceptance_criteria: vec![
                NonEmptyString::parse("acceptance_criteria", "Tests pass").unwrap(),
            ],
            verification_commands: vec![
                NonEmptyString::parse("verification_commands", "cargo test products").unwrap(),
            ],
            limits: TaskLimits {
                max_input_tokens: PositiveLimit::new("max_input_tokens", 30_000).unwrap(),
                max_output_tokens: PositiveLimit::new("max_output_tokens", 8_000).unwrap(),
                max_tool_calls: PositiveLimit::new("max_tool_calls", 40).unwrap(),
                max_attempts: MaxAttempts::new(2).unwrap(),
                timeout_seconds: PositiveLimit::new("timeout_seconds", 1_200).unwrap(),
            },
        }
    }

    #[test]
    fn ready_requires_scheduling_fields() {
        let mut task = contract();
        assert!(task.validate_for_status(TaskStatus::Ready).is_ok());
        task.acceptance_criteria.clear();
        assert_eq!(
            task.validate_for_status(TaskStatus::Ready)
                .unwrap_err()
                .field,
            "acceptance_criteria"
        );
        assert!(task.validate_for_status(TaskStatus::Draft).is_ok());

        let mut task = contract();
        task.allowed_paths.clear();
        assert_eq!(
            task.validate_for_status(TaskStatus::Ready)
                .unwrap_err()
                .field,
            "allowed_paths"
        );

        let mut task = contract();
        task.verification_commands.clear();
        assert_eq!(
            task.validate_for_status(TaskStatus::Ready)
                .unwrap_err()
                .field,
            "verification_commands"
        );
    }

    #[test]
    fn paths_reject_empty_absolute_and_traversal() {
        for path in [
            "",
            " ",
            "/etc/passwd",
            "../secret",
            "src/../../secret",
            r"C:\\secret",
            r"..\\secret",
            r"\\server\\share",
            ".",
            "./README.md",
            "src/secret\n",
        ] {
            assert!(AllowedPath::parse(path).is_err(), "accepted {path}");
        }
        for path in ["src/**", "tests/task.rs", "README.md"] {
            assert!(AllowedPath::parse(path).is_ok(), "rejected {path}");
        }
    }

    #[test]
    fn limits_enforce_positive_json_safe_values_and_attempt_ceiling() {
        for value in [0, -1, MAX_SAFE_INTEGER + 1] {
            assert!(PositiveLimit::new("max_input_tokens", value).is_err());
        }
        assert!(PositiveLimit::new("max_input_tokens", MAX_SAFE_INTEGER).is_ok());
        for value in [0, -1, 11] {
            assert!(MaxAttempts::new(value).is_err());
        }
        assert!(MaxAttempts::new(1).is_ok());
        assert!(MaxAttempts::new(10).is_ok());
    }

    #[test]
    fn scalar_values_reject_whitespace() {
        assert!(NonEmptyString::parse("id", " ").is_err());
        assert!(NonEmptyString::parse("title", "title").is_ok());
    }
}
