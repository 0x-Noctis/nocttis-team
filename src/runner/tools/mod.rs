use std::{
    fmt, fs,
    path::{Path, PathBuf},
    time::Instant,
};

use crate::{
    runner::git::{GitError, GitWorktreeManager, Worktree},
    store::artifact::{ArtifactError, ArtifactMetadata, ArtifactStore},
};

use super::policy::{PolicyError, ToolName, ToolPolicy, ToolRole, validate_relative};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolRequest {
    ListFiles {
        path: String,
    },
    SearchCode {
        path: String,
        query: String,
    },
    ReadFile {
        path: String,
    },
    ApplyPatch {
        patch: Vec<u8>,
    },
    GitDiff,
    GitStatus,
    SubmitArtifact {
        path: String,
        artifact_id: String,
        logical_name: String,
        media_type: String,
    },
    RequestHuman {
        message: String,
    },
}

impl ToolRequest {
    pub fn name(&self) -> ToolName {
        match self {
            Self::ListFiles { .. } => ToolName::ListFiles,
            Self::SearchCode { .. } => ToolName::SearchCode,
            Self::ReadFile { .. } => ToolName::ReadFile,
            Self::ApplyPatch { .. } => ToolName::ApplyPatch,
            Self::GitDiff => ToolName::GitDiff,
            Self::GitStatus => ToolName::GitStatus,
            Self::SubmitArtifact { .. } => ToolName::SubmitArtifact,
            Self::RequestHuman { .. } => ToolName::RequestHuman,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolResult {
    Files(Vec<String>),
    SearchMatches(Vec<SearchMatch>),
    File(Vec<u8>),
    PatchApplied,
    Diff(Vec<u8>),
    Status(String),
    Artifact(ArtifactMetadata),
    HumanRequested { message: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchMatch {
    pub path: String,
    pub line: usize,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolErrorCode {
    Denied,
    InvalidArgument,
    UnsafePath,
    TooLarge,
    Timeout,
    BinaryInput,
    PatchDenied,
    NotFound,
    OperationFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolError {
    pub code: ToolErrorCode,
    pub message: &'static str,
}

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}
impl std::error::Error for ToolError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditEvent {
    pub tool: ToolName,
    pub role: ToolRole,
    pub succeeded: bool,
    pub error_code: Option<ToolErrorCode>,
    pub duration_ms: u64,
}

#[derive(Debug)]
pub struct ToolExecution {
    pub result: Result<ToolResult, ToolError>,
    pub audit: AuditEvent,
}

pub struct StructuredTools<'a> {
    policy: ToolPolicy,
    git: &'a GitWorktreeManager,
    worktree: &'a Worktree,
    artifacts: &'a ArtifactStore,
}

impl<'a> StructuredTools<'a> {
    pub fn new(
        policy: ToolPolicy,
        git: &'a GitWorktreeManager,
        worktree: &'a Worktree,
        artifacts: &'a ArtifactStore,
    ) -> Self {
        Self {
            policy,
            git,
            worktree,
            artifacts,
        }
    }

    pub fn execute(&self, request: ToolRequest) -> ToolExecution {
        let started = Instant::now();
        let tool = request.name();
        let result = self
            .policy
            .authorize_tool(tool)
            .map_err(map_policy)
            .and_then(|()| self.execute_authorized(request, started));
        let result = if started.elapsed() > self.policy.timeout() {
            Err(error(ToolErrorCode::Timeout, "tool call timed out"))
        } else {
            result
        };
        ToolExecution {
            audit: AuditEvent {
                tool,
                role: self.policy.role(),
                succeeded: result.is_ok(),
                error_code: result.as_ref().err().map(|error| error.code),
                duration_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
            },
            result,
        }
    }

    fn execute_authorized(
        &self,
        request: ToolRequest,
        started: Instant,
    ) -> Result<ToolResult, ToolError> {
        match request {
            ToolRequest::ListFiles { path } => self.list_files(&path, started),
            ToolRequest::SearchCode { path, query } => self.search_code(&path, &query, started),
            ToolRequest::ReadFile { path } => self.read_file(&path).map(ToolResult::File),
            ToolRequest::ApplyPatch { patch } => self.apply_patch(&patch),
            ToolRequest::GitDiff => bounded(
                self.git.diff_binary(self.worktree).map_err(map_git)?,
                self.policy.maximum_output_bytes(),
            )
            .map(ToolResult::Diff),
            ToolRequest::GitStatus => {
                let bytes = bounded(
                    self.git
                        .status(self.worktree)
                        .map_err(map_git)?
                        .into_bytes(),
                    self.policy.maximum_output_bytes(),
                )?;
                Ok(ToolResult::Status(String::from_utf8(bytes).map_err(
                    |_| error(ToolErrorCode::OperationFailed, "tool output is invalid"),
                )?))
            }
            ToolRequest::SubmitArtifact {
                path,
                artifact_id,
                logical_name,
                media_type,
            } => self.submit_artifact(&path, &artifact_id, &logical_name, &media_type),
            ToolRequest::RequestHuman { message } => {
                self.policy
                    .check_argument(message.as_bytes())
                    .map_err(map_policy)?;
                Ok(ToolResult::HumanRequested { message })
            }
        }
    }

    fn list_files(&self, path: &str, started: Instant) -> Result<ToolResult, ToolError> {
        self.policy.authorize_path(path).map_err(map_policy)?;
        let root = safe_existing(self.worktree.path(), path, true)?;
        let mut files = Vec::new();
        collect_files(
            self.worktree.path(),
            &root,
            &mut files,
            started,
            self.policy.timeout(),
        )?;
        files.sort();
        let bytes = bounded(
            files.join("\n").into_bytes(),
            self.policy.maximum_output_bytes(),
        )?;
        let text = String::from_utf8(bytes)
            .map_err(|_| error(ToolErrorCode::OperationFailed, "tool output is invalid"))?;
        Ok(ToolResult::Files(if text.is_empty() {
            Vec::new()
        } else {
            text.lines().map(str::to_owned).collect()
        }))
    }

    fn search_code(
        &self,
        path: &str,
        query: &str,
        started: Instant,
    ) -> Result<ToolResult, ToolError> {
        self.policy.authorize_path(path).map_err(map_policy)?;
        self.policy
            .check_argument(query.as_bytes())
            .map_err(map_policy)?;
        if query.is_empty() || query.starts_with('-') || query.chars().any(char::is_control) {
            return Err(error(
                ToolErrorCode::InvalidArgument,
                "search query is invalid",
            ));
        }
        let root = safe_existing(self.worktree.path(), path, true)?;
        let mut files = Vec::new();
        collect_files(
            self.worktree.path(),
            &root,
            &mut files,
            started,
            self.policy.timeout(),
        )?;
        let mut matches = Vec::new();
        let mut output_size = 0_usize;
        for relative in files {
            check_timeout(started, self.policy.timeout())?;
            let bytes = fs::read(self.worktree.path().join(&relative))
                .map_err(|_| error(ToolErrorCode::OperationFailed, "tool read failed"))?;
            if bytes.contains(&0) {
                continue;
            }
            let text = String::from_utf8(bytes)
                .map_err(|_| error(ToolErrorCode::BinaryInput, "binary input is denied"))?;
            for (index, line) in text.lines().enumerate() {
                if line.contains(query) {
                    output_size = output_size.saturating_add(relative.len() + line.len() + 16);
                    if output_size > self.policy.maximum_output_bytes() {
                        return Err(error(ToolErrorCode::TooLarge, "tool output exceeds limit"));
                    }
                    matches.push(SearchMatch {
                        path: relative.clone(),
                        line: index + 1,
                        text: line.to_owned(),
                    });
                }
            }
        }
        Ok(ToolResult::SearchMatches(matches))
    }

    fn read_file(&self, path: &str) -> Result<Vec<u8>, ToolError> {
        self.policy.authorize_path(path).map_err(map_policy)?;
        let path = safe_existing(self.worktree.path(), path, false)?;
        let bytes = fs::read(path)
            .map_err(|_| error(ToolErrorCode::OperationFailed, "tool read failed"))?;
        if bytes.contains(&0) {
            return Err(error(ToolErrorCode::BinaryInput, "binary input is denied"));
        }
        bounded(bytes, self.policy.maximum_output_bytes())
    }

    fn apply_patch(&self, patch: &[u8]) -> Result<ToolResult, ToolError> {
        self.policy.check_argument(patch).map_err(map_policy)?;
        if patch.contains(&0)
            || patch.windows(16).any(|part| part == b"GIT binary patch")
            || patch.windows(12).any(|part| part == b"Binary files")
        {
            return Err(error(ToolErrorCode::BinaryInput, "binary patch is denied"));
        }
        let text = std::str::from_utf8(patch)
            .map_err(|_| error(ToolErrorCode::BinaryInput, "binary patch is denied"))?;
        let paths = patch_paths(text)?;
        if paths.is_empty() {
            return Err(error(ToolErrorCode::InvalidArgument, "patch is invalid"));
        }
        for path in paths {
            self.policy
                .authorize_path(&path)
                .map_err(|_| error(ToolErrorCode::PatchDenied, "patch touches denied path"))?;
            safe_parent(self.worktree.path(), &path)?;
        }
        self.git
            .apply_patch(self.worktree, patch)
            .map_err(map_git)?;
        Ok(ToolResult::PatchApplied)
    }

    fn submit_artifact(
        &self,
        path: &str,
        artifact_id: &str,
        logical_name: &str,
        media_type: &str,
    ) -> Result<ToolResult, ToolError> {
        let bytes = self.read_file(path)?;
        self.policy
            .check_argument(artifact_id.as_bytes())
            .map_err(map_policy)?;
        self.policy
            .check_argument(logical_name.as_bytes())
            .map_err(map_policy)?;
        self.policy
            .check_argument(media_type.as_bytes())
            .map_err(map_policy)?;
        self.artifacts
            .write(artifact_id, logical_name, media_type, &bytes, |_| {
                Ok::<(), ()>(())
            })
            .map(ToolResult::Artifact)
            .map_err(map_artifact)
    }
}

fn safe_existing(root: &Path, relative: &str, directory: bool) -> Result<PathBuf, ToolError> {
    let relative = validate_relative(relative).map_err(map_policy)?;
    let root = root
        .canonicalize()
        .map_err(|_| error(ToolErrorCode::OperationFailed, "worktree is unavailable"))?;
    let path = root
        .join(relative)
        .canonicalize()
        .map_err(|_| error(ToolErrorCode::NotFound, "tool path was not found"))?;
    if !path.starts_with(&root) || (directory && !path.is_dir()) || (!directory && !path.is_file())
    {
        return Err(error(ToolErrorCode::UnsafePath, "tool path is unsafe"));
    }
    Ok(path)
}

fn safe_parent(root: &Path, relative: &str) -> Result<(), ToolError> {
    let relative = validate_relative(relative).map_err(map_policy)?;
    let root = root
        .canonicalize()
        .map_err(|_| error(ToolErrorCode::OperationFailed, "worktree is unavailable"))?;
    let parent = root
        .join(relative)
        .parent()
        .ok_or_else(|| error(ToolErrorCode::UnsafePath, "tool path is unsafe"))?
        .canonicalize()
        .map_err(|_| error(ToolErrorCode::UnsafePath, "tool path is unsafe"))?;
    if !parent.starts_with(root) {
        return Err(error(ToolErrorCode::UnsafePath, "tool path is unsafe"));
    }
    Ok(())
}

fn collect_files(
    root: &Path,
    current: &Path,
    files: &mut Vec<String>,
    started: Instant,
    timeout: std::time::Duration,
) -> Result<(), ToolError> {
    check_timeout(started, timeout)?;
    for entry in fs::read_dir(current)
        .map_err(|_| error(ToolErrorCode::OperationFailed, "tool listing failed"))?
    {
        check_timeout(started, timeout)?;
        let entry =
            entry.map_err(|_| error(ToolErrorCode::OperationFailed, "tool listing failed"))?;
        let metadata = entry
            .file_type()
            .map_err(|_| error(ToolErrorCode::OperationFailed, "tool listing failed"))?;
        if metadata.is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_files(root, &entry.path(), files, started, timeout)?;
        } else if metadata.is_file() {
            files.push(
                entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|_| error(ToolErrorCode::UnsafePath, "tool path is unsafe"))?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

fn patch_paths(patch: &str) -> Result<Vec<String>, ToolError> {
    let mut paths = Vec::new();
    for line in patch.lines() {
        if let Some(raw) = line
            .strip_prefix("+++ ")
            .or_else(|| line.strip_prefix("--- "))
        {
            if raw == "/dev/null" {
                continue;
            }
            let raw = raw
                .strip_prefix("a/")
                .or_else(|| raw.strip_prefix("b/"))
                .ok_or_else(|| error(ToolErrorCode::InvalidArgument, "patch path is invalid"))?;
            if raw.contains('\t') || raw.contains(' ') {
                return Err(error(
                    ToolErrorCode::InvalidArgument,
                    "patch path is invalid",
                ));
            }
            paths.push(validate_relative(raw).map_err(map_policy)?);
        }
    }
    Ok(paths)
}

fn bounded(bytes: Vec<u8>, limit: usize) -> Result<Vec<u8>, ToolError> {
    (bytes.len() <= limit)
        .then_some(bytes)
        .ok_or_else(|| error(ToolErrorCode::TooLarge, "tool output exceeds limit"))
}
fn check_timeout(started: Instant, timeout: std::time::Duration) -> Result<(), ToolError> {
    (started.elapsed() <= timeout)
        .then_some(())
        .ok_or_else(|| error(ToolErrorCode::Timeout, "tool call timed out"))
}
fn map_policy(error_value: PolicyError) -> ToolError {
    match error_value {
        PolicyError::DeniedTool | PolicyError::DeniedPath => {
            error(ToolErrorCode::Denied, "tool call denied by policy")
        }
        PolicyError::ArgumentTooLarge => {
            error(ToolErrorCode::TooLarge, "tool argument exceeds limit")
        }
        PolicyError::InvalidPath | PolicyError::InvalidAllowedPath => {
            error(ToolErrorCode::UnsafePath, "tool path is unsafe")
        }
        _ => error(ToolErrorCode::InvalidArgument, "tool argument is invalid"),
    }
}
fn map_git(_: GitError) -> ToolError {
    error(ToolErrorCode::OperationFailed, "git operation failed")
}
fn map_artifact(error_value: ArtifactError) -> ToolError {
    match error_value {
        ArtifactError::TooLarge => error(ToolErrorCode::TooLarge, "artifact exceeds limit"),
        ArtifactError::UnsafePath => error(ToolErrorCode::UnsafePath, "artifact path is unsafe"),
        ArtifactError::NotFound => error(ToolErrorCode::NotFound, "artifact was not found"),
        _ => error(ToolErrorCode::OperationFailed, "artifact operation failed"),
    }
}
fn error(code: ToolErrorCode, message: &'static str) -> ToolError {
    ToolError { code, message }
}
