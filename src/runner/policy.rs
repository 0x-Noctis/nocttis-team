use std::{
    fmt,
    path::{Component, Path},
    time::Duration,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolRole {
    Worker,
    Reviewer,
    Verifier,
    Architect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolName {
    ListFiles,
    SearchCode,
    ReadFile,
    ApplyPatch,
    GitDiff,
    GitStatus,
    SubmitArtifact,
    RequestHuman,
}

impl fmt::Display for ToolName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ListFiles => "list_files",
            Self::SearchCode => "search_code",
            Self::ReadFile => "read_file",
            Self::ApplyPatch => "apply_patch",
            Self::GitDiff => "git_diff",
            Self::GitStatus => "git_status",
            Self::SubmitArtifact => "submit_artifact",
            Self::RequestHuman => "request_human",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyError {
    EmptyAllowedPaths,
    InvalidAllowedPath,
    InvalidLimit,
    DeniedTool,
    InvalidPath,
    DeniedPath,
    ArgumentTooLarge,
}

impl fmt::Display for PolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyAllowedPaths => "tool policy requires allowed paths",
            Self::InvalidAllowedPath => "tool policy contains an invalid allowed path",
            Self::InvalidLimit => "tool policy limits must be positive",
            Self::DeniedTool => "tool is denied for role",
            Self::InvalidPath => "tool path is invalid",
            Self::DeniedPath => "tool path is outside allowed paths",
            Self::ArgumentTooLarge => "tool argument exceeds limit",
        })
    }
}

impl std::error::Error for PolicyError {}

#[derive(Clone, Debug)]
pub struct ToolPolicy {
    role: ToolRole,
    allowed_paths: Vec<String>,
    maximum_argument_bytes: usize,
    maximum_output_bytes: usize,
    timeout: Duration,
}

impl ToolPolicy {
    pub fn new(
        role: ToolRole,
        allowed_paths: Vec<String>,
        maximum_argument_bytes: usize,
        maximum_output_bytes: usize,
        timeout: Duration,
    ) -> Result<Self, PolicyError> {
        if allowed_paths.is_empty() {
            return Err(PolicyError::EmptyAllowedPaths);
        }
        if maximum_argument_bytes == 0 || maximum_output_bytes == 0 || timeout.is_zero() {
            return Err(PolicyError::InvalidLimit);
        }
        for path in &allowed_paths {
            validate_relative(path).map_err(|_| PolicyError::InvalidAllowedPath)?;
        }
        Ok(Self {
            role,
            allowed_paths,
            maximum_argument_bytes,
            maximum_output_bytes,
            timeout,
        })
    }

    pub fn role(&self) -> ToolRole {
        self.role
    }
    pub fn maximum_output_bytes(&self) -> usize {
        self.maximum_output_bytes
    }
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn authorize_tool(&self, tool: ToolName) -> Result<(), PolicyError> {
        let allowed = match self.role {
            ToolRole::Worker => true,
            ToolRole::Reviewer | ToolRole::Verifier => {
                !matches!(tool, ToolName::ApplyPatch | ToolName::SubmitArtifact)
            }
            ToolRole::Architect => matches!(
                tool,
                ToolName::ListFiles
                    | ToolName::SearchCode
                    | ToolName::ReadFile
                    | ToolName::RequestHuman
            ),
        };
        allowed.then_some(()).ok_or(PolicyError::DeniedTool)
    }

    pub fn authorize_path(&self, path: &str) -> Result<(), PolicyError> {
        let normalized = validate_relative(path)?;
        self.allowed_paths
            .iter()
            .any(|allowed| path_matches(allowed, &normalized))
            .then_some(())
            .ok_or(PolicyError::DeniedPath)
    }

    pub fn check_argument(&self, value: &[u8]) -> Result<(), PolicyError> {
        (value.len() <= self.maximum_argument_bytes)
            .then_some(())
            .ok_or(PolicyError::ArgumentTooLarge)
    }
}

pub fn validate_relative(path: &str) -> Result<String, PolicyError> {
    if path.is_empty() || path.starts_with('-') || path.chars().any(char::is_control) {
        return Err(PolicyError::InvalidPath);
    }
    let normalized = path.replace('\\', "/");
    if normalized.starts_with('/')
        || normalized.starts_with("//")
        || normalized.as_bytes().get(1) == Some(&b':')
    {
        return Err(PolicyError::InvalidPath);
    }
    if Path::new(&normalized)
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(PolicyError::InvalidPath);
    }
    Ok(normalized)
}

fn path_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    if let Some(prefix) = pattern.strip_suffix("/**") {
        path == prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
    } else if let Some(prefix) = pattern.strip_suffix("/*") {
        path.strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/') && !rest[1..].contains('/'))
    } else {
        path == pattern
    }
}
