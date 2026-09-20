use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    agent::worker::WorkerHandoff,
    domain::{
        state_machine::{Actor, transition},
        task::{TaskContract, TaskStatus},
    },
    model::{Message, MessageRole, ModelLimits, ModelRequest, ModelResponse},
    runner::git::{GitWorktreeManager, Worktree},
    store::artifact::ArtifactStore,
};

const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_FINDINGS: usize = 64;
const MAX_MESSAGE_BYTES: usize = 1_024;
const MAX_SOURCE_BYTES: usize = 64 * 1024;
const MAX_EVIDENCE_BYTES: u64 = 64 * 1024;
const MAX_UNTRACKED_FILES: usize = 1_024;
const MAX_UNTRACKED_FILE_BYTES: u64 = 1024 * 1024;
const MAX_UNTRACKED_TOTAL_BYTES: u64 = 8 * 1024 * 1024;
const MAX_UNTRACKED_PATH_BYTES: usize = 4 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Finding {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub path: Option<String>,
    pub line: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "decision")]
pub enum ReviewDecision {
    Approved,
    ChangesRequested { findings: Vec<Finding> },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReviewOutcome {
    pub decision: ReviewDecision,
    pub next_status: TaskStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceExcerpt {
    pub path: String,
    pub start_line: u64,
    pub content: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationEvidence {
    pub command: String,
    pub succeeded: bool,
    pub artifact_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiffMetadata {
    pub bytes: usize,
    pub binary: bool,
    pub checksum: String,
    pub status: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReviewerError {
    Git,
    Artifact,
    InputTooLarge,
    Model,
    ResponseTooLarge,
    InvalidResponse,
    InvalidFinding,
    MutationInspection,
    PolicyViolation,
    InvalidTransition,
}

impl fmt::Display for ReviewerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Git => "review git snapshot failed",
            Self::Artifact => "review evidence unavailable",
            Self::InputTooLarge => "review input exceeds limit",
            Self::Model => "review model call failed",
            Self::ResponseTooLarge => "review response exceeds limit",
            Self::InvalidResponse => "review response is invalid",
            Self::InvalidFinding => "review finding is invalid",
            Self::MutationInspection => "worktree mutation inspection failed",
            Self::PolicyViolation => "reviewer modified worktree",
            Self::InvalidTransition => "review transition is invalid",
        })
    }
}
impl std::error::Error for ReviewerError {}

pub trait ReviewerModel {
    type Error;
    fn complete(&mut self, request: &ModelRequest) -> Result<ModelResponse, Self::Error>;
}

pub struct Reviewer<'a, M> {
    contract: &'a TaskContract,
    handoff: &'a WorkerHandoff,
    git: &'a GitWorktreeManager,
    worktree: &'a Worktree,
    artifacts: &'a ArtifactStore,
    model: M,
    agent_run_id: String,
    model_class: String,
}

impl<'a, M: ReviewerModel> Reviewer<'a, M> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        contract: &'a TaskContract,
        handoff: &'a WorkerHandoff,
        git: &'a GitWorktreeManager,
        worktree: &'a Worktree,
        artifacts: &'a ArtifactStore,
        model: M,
        agent_run_id: impl Into<String>,
        model_class: impl Into<String>,
    ) -> Self {
        Self {
            contract,
            handoff,
            git,
            worktree,
            artifacts,
            model,
            agent_run_id: agent_run_id.into(),
            model_class: model_class.into(),
        }
    }

    pub fn review(
        mut self,
        sources: &[SourceExcerpt],
        verification: &[VerificationEvidence],
    ) -> ReviewRun<M> {
        let before = match self.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => return self.finish(None, Some(error)),
        };
        let request = match self.request(&before, sources, verification) {
            Ok(request) => request,
            Err(error) => return self.finish(None, Some(error)),
        };
        let response = self.model.complete(&request);
        let after = match self.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => return self.finish(None, Some(error)),
        };
        if before != after {
            return self.finish(None, Some(ReviewerError::PolicyViolation));
        }
        let response = match response {
            Ok(response) => response,
            Err(_) => return self.finish(None, Some(ReviewerError::Model)),
        };
        let decision = match self.parse(response) {
            Ok(decision) => decision,
            Err(error) => return self.finish(None, Some(error)),
        };
        let next_status = match decision {
            ReviewDecision::Approved => TaskStatus::Verify,
            ReviewDecision::ChangesRequested { .. } => TaskStatus::ChangesRequested,
        };
        if transition(
            self.contract,
            TaskStatus::Review,
            next_status,
            Actor::Reviewer,
        )
        .is_err()
        {
            return self.finish(None, Some(ReviewerError::InvalidTransition));
        }
        self.finish(
            Some(ReviewOutcome {
                decision,
                next_status,
            }),
            None,
        )
    }

    fn snapshot(&self) -> Result<GitSnapshot, ReviewerError> {
        let status = self
            .git
            .status(self.worktree)
            .map_err(|_| ReviewerError::Git)?;
        Ok(GitSnapshot {
            untracked: snapshot_untracked(self.worktree.path(), &status)?,
            status,
            diff: self
                .git
                .diff_binary(self.worktree)
                .map_err(|_| ReviewerError::Git)?,
        })
    }

    fn request(
        &self,
        snapshot: &GitSnapshot,
        sources: &[SourceExcerpt],
        verification: &[VerificationEvidence],
    ) -> Result<ModelRequest, ReviewerError> {
        let mut source_bytes = 0_usize;
        for source in sources {
            validate_path(self.contract, &source.path)?;
            source_bytes = source_bytes
                .checked_add(source.content.len())
                .ok_or(ReviewerError::InputTooLarge)?;
            if source_bytes > MAX_SOURCE_BYTES {
                return Err(ReviewerError::InputTooLarge);
            }
        }
        let mut artifact_evidence = Vec::new();
        for evidence in verification {
            if unsafe_text(&evidence.command) {
                return Err(ReviewerError::InvalidResponse);
            }
            let content = match &evidence.artifact_id {
                Some(id) => {
                    let bytes = self
                        .artifacts
                        .read_bounded(id, MAX_EVIDENCE_BYTES)
                        .map_err(|_| ReviewerError::Artifact)?;
                    String::from_utf8(bytes).unwrap_or_else(|_| "[binary evidence]".into())
                }
                None => String::new(),
            };
            artifact_evidence.push(json!({
                "command": evidence.command,
                "succeeded": evidence.succeeded,
                "artifact_id": evidence.artifact_id,
                "content": content,
            }));
        }
        let metadata = DiffMetadata {
            bytes: snapshot.diff.len(),
            binary: snapshot
                .diff
                .windows(b"GIT binary patch".len())
                .any(|window| window == b"GIT binary patch"),
            checksum: hex(&Sha256::digest(&snapshot.diff)),
            status: snapshot.status.clone(),
        };
        let content = serde_json::to_string(&json!({
            "task": self.contract,
            "worker_handoff": self.handoff,
            "diff": metadata,
            "source_excerpts": sources.iter().map(|source| json!({
                "path": source.path,
                "start_line": source.start_line,
                "content": source.content,
            })).collect::<Vec<_>>(),
            "verification": artifact_evidence,
            "required_output": {
                "decision": "approved | changes_requested",
                "findings": "required and non-empty only for changes_requested",
            }
        }))
        .map_err(|_| ReviewerError::InputTooLarge)?;
        if content.len() > MAX_SOURCE_BYTES + MAX_EVIDENCE_BYTES as usize + MAX_RESPONSE_BYTES {
            return Err(ReviewerError::InputTooLarge);
        }
        Ok(ModelRequest {
            project_id: self.contract.project_id.as_str().to_owned(),
            task_id: self.contract.id.as_str().to_owned(),
            agent_run_id: self.agent_run_id.clone(),
            model_class: self.model_class.clone(),
            messages: vec![Message {
                role: MessageRole::User,
                content,
                tool_call_id: None,
            }],
            tools: Vec::new(),
            limits: ModelLimits {
                max_input_tokens: self
                    .contract
                    .limits
                    .max_input_tokens
                    .get()
                    .try_into()
                    .unwrap_or(0),
                max_output_tokens: self
                    .contract
                    .limits
                    .max_output_tokens
                    .get()
                    .try_into()
                    .unwrap_or(0),
            },
        })
    }

    fn parse(&self, response: ModelResponse) -> Result<ReviewDecision, ReviewerError> {
        if !response.tool_calls.is_empty() {
            return Err(ReviewerError::InvalidResponse);
        }
        let body = response.content.ok_or(ReviewerError::InvalidResponse)?;
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(ReviewerError::ResponseTooLarge);
        }
        let raw: RawDecision =
            serde_json::from_str(&body).map_err(|_| ReviewerError::InvalidResponse)?;
        match raw.decision {
            RawDecisionKind::Approved => {
                if !raw.findings.is_empty() {
                    return Err(ReviewerError::InvalidResponse);
                }
                Ok(ReviewDecision::Approved)
            }
            RawDecisionKind::ChangesRequested => {
                if raw.findings.is_empty() || raw.findings.len() > MAX_FINDINGS {
                    return Err(ReviewerError::InvalidResponse);
                }
                let findings = raw
                    .findings
                    .into_iter()
                    .map(|finding| self.validate_finding(finding))
                    .collect::<Result<_, _>>()?;
                Ok(ReviewDecision::ChangesRequested { findings })
            }
        }
    }

    fn validate_finding(&self, finding: RawFinding) -> Result<Finding, ReviewerError> {
        if finding.code.is_empty()
            || finding.code.len() > 64
            || !finding
                .code
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
            || finding.message.is_empty()
            || finding.message.len() > MAX_MESSAGE_BYTES
            || unsafe_text(&finding.message)
            || finding.line == Some(0)
        {
            return Err(ReviewerError::InvalidFinding);
        }
        if let Some(path) = &finding.path {
            validate_path(self.contract, path)?;
        }
        Ok(Finding {
            severity: finding.severity,
            code: finding.code,
            message: finding.message,
            path: finding.path,
            line: finding.line,
        })
    }

    fn finish(self, outcome: Option<ReviewOutcome>, error: Option<ReviewerError>) -> ReviewRun<M> {
        ReviewRun {
            outcome,
            error,
            model: self.model,
        }
    }
}

pub struct ReviewRun<M> {
    pub outcome: Option<ReviewOutcome>,
    pub error: Option<ReviewerError>,
    pub model: M,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDecision {
    decision: RawDecisionKind,
    #[serde(default)]
    findings: Vec<RawFinding>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawDecisionKind {
    Approved,
    ChangesRequested,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFinding {
    severity: Severity,
    code: String,
    message: String,
    path: Option<String>,
    line: Option<u64>,
}

impl<'de> Deserialize<'de> for Severity {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match String::deserialize(deserializer)?.as_str() {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "critical" => Ok(Self::Critical),
            _ => Err(serde::de::Error::custom("invalid severity")),
        }
    }
}

#[derive(Eq, PartialEq)]
struct GitSnapshot {
    status: String,
    diff: Vec<u8>,
    untracked: BTreeMap<String, UntrackedFile>,
}

#[derive(Eq, PartialEq)]
struct UntrackedFile {
    size: u64,
    checksum: [u8; 32],
}

fn snapshot_untracked(
    worktree: &Path,
    status: &str,
) -> Result<BTreeMap<String, UntrackedFile>, ReviewerError> {
    let root = worktree
        .canonicalize()
        .map_err(|_| ReviewerError::MutationInspection)?;
    if !root.is_dir() {
        return Err(ReviewerError::MutationInspection);
    }
    let mut paths = status
        .lines()
        .filter_map(|line| line.strip_prefix("?? "))
        .collect::<Vec<_>>();
    paths.sort_unstable();
    paths.dedup();
    if paths.len() > MAX_UNTRACKED_FILES {
        return Err(ReviewerError::MutationInspection);
    }
    let mut total = 0_u64;
    let mut files = BTreeMap::new();
    for relative in paths {
        validate_untracked_path(relative)?;
        let path = safe_untracked_path(&root, relative)?;
        let metadata = fs::metadata(&path).map_err(|_| ReviewerError::MutationInspection)?;
        if !metadata.is_file() || metadata.len() > MAX_UNTRACKED_FILE_BYTES {
            return Err(ReviewerError::MutationInspection);
        }
        total = total
            .checked_add(metadata.len())
            .ok_or(ReviewerError::MutationInspection)?;
        if total > MAX_UNTRACKED_TOTAL_BYTES {
            return Err(ReviewerError::MutationInspection);
        }
        let mut file = File::open(&path).map_err(|_| ReviewerError::MutationInspection)?;
        let mut bytes = Vec::with_capacity(metadata.len().try_into().unwrap_or(0));
        file.by_ref()
            .take(MAX_UNTRACKED_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ReviewerError::MutationInspection)?;
        if bytes.len() as u64 != metadata.len() {
            return Err(ReviewerError::MutationInspection);
        }
        files.insert(
            relative.to_owned(),
            UntrackedFile {
                size: metadata.len(),
                checksum: Sha256::digest(bytes).into(),
            },
        );
    }
    Ok(files)
}

fn validate_untracked_path(value: &str) -> Result<(), ReviewerError> {
    if value.is_empty()
        || value.len() > MAX_UNTRACKED_PATH_BYTES
        || value.starts_with('"')
        || value.starts_with('-')
        || value.starts_with('/')
        || value.starts_with("//")
        || value.as_bytes().get(1) == Some(&b':')
        || value.chars().any(char::is_control)
        || Path::new(value)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ReviewerError::MutationInspection);
    }
    Ok(())
}

fn safe_untracked_path(root: &Path, relative: &str) -> Result<PathBuf, ReviewerError> {
    let mut current = root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(component) = component else {
            return Err(ReviewerError::MutationInspection);
        };
        current.push(component);
        let metadata =
            fs::symlink_metadata(&current).map_err(|_| ReviewerError::MutationInspection)?;
        if metadata.file_type().is_symlink() {
            return Err(ReviewerError::MutationInspection);
        }
    }
    let canonical = current
        .canonicalize()
        .map_err(|_| ReviewerError::MutationInspection)?;
    if !canonical.starts_with(root) {
        return Err(ReviewerError::MutationInspection);
    }
    Ok(canonical)
}

fn validate_path(contract: &TaskContract, value: &str) -> Result<(), ReviewerError> {
    let normalized = value.replace('\\', "/");
    if normalized.is_empty()
        || normalized.starts_with('-')
        || normalized.starts_with('/')
        || normalized.starts_with("//")
        || normalized.as_bytes().get(1) == Some(&b':')
        || normalized.chars().any(char::is_control)
        || std::path::Path::new(&normalized)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || !contract
            .allowed_paths
            .iter()
            .any(|allowed| path_matches(allowed.as_str(), &normalized))
    {
        return Err(ReviewerError::InvalidFinding);
    }
    Ok(())
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
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|rest| !rest.is_empty() && !rest.contains('/'))
    } else {
        pattern == path
    }
}

fn unsafe_text(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.chars().any(char::is_control)
        || value.split_whitespace().any(|word| {
            word.starts_with('/')
                || word.starts_with("\\\\")
                || word.as_bytes().get(1) == Some(&b':')
        })
        || ["api_key", "password", "secret", "bearer ", "token="]
            .iter()
            .any(|marker| lower.contains(marker))
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

#[cfg(test)]
mod tests {
    use super::{ReviewerError, validate_untracked_path};

    #[test]
    fn synthetic_overlong_untracked_path_is_typed_and_sanitized() {
        let secret = "PATH_SECRET_MARKER";
        let path = format!("{secret}{}", "x".repeat(4097));
        let error = validate_untracked_path(&path).unwrap_err();

        assert_eq!(error, ReviewerError::MutationInspection);
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains(secret));
        assert!(!rendered.contains("/absolute/host/path"));
        assert!(!rendered.contains(&path));
    }
}
