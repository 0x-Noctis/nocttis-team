use std::{
    fmt, fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

use sha2::{Digest, Sha256};

use crate::{
    domain::{
        state_machine::{Actor, transition},
        task::{TaskContract, TaskStatus},
    },
    runner::process::{
        ArtifactTargets, CommandRequest, CommandResult, ProcessError, ProcessRunner, StoredOutput,
        VerificationCommand,
    },
    store::artifact::ArtifactStore,
};

const MAX_COMMANDS: usize = 128;
const MAX_COMMAND_BYTES: usize = 4_096;
const MAX_GIT_SNAPSHOT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerificationVerdict {
    Integrate,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationResult {
    pub command: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub timed_out: bool,
    pub stdout_artifact_id: String,
    pub stderr_artifact_id: String,
    pub passed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationReport {
    pub verdict: VerificationVerdict,
    pub proposed_status: TaskStatus,
    pub results: Vec<VerificationResult>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerifierError {
    EmptyVerificationList,
    InvalidCommand,
    CommandDenied,
    ExecutionFailed,
    ArtifactFailed,
    SourceMutation,
    GitInspectionFailed,
    InvalidTransition,
}

impl fmt::Display for VerifierError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyVerificationList => "verification command list is empty",
            Self::InvalidCommand => "verification command is invalid",
            Self::CommandDenied => "verification command is not allowed",
            Self::ExecutionFailed => "verification command execution failed",
            Self::ArtifactFailed => "verification artifact persistence failed",
            Self::SourceMutation => "verification modified repository source",
            Self::GitInspectionFailed => "repository inspection failed",
            Self::InvalidTransition => "verifier transition is invalid",
        })
    }
}

impl std::error::Error for VerifierError {}

pub trait ProcessExecutor {
    fn execute(
        &mut self,
        request: &CommandRequest,
        targets: &ArtifactTargets,
    ) -> Result<(CommandResult, StoredOutput), ProcessError>;
}

pub struct ProductionExecutor<'a> {
    runner: &'a ProcessRunner,
    artifacts: &'a ArtifactStore,
}

impl<'a> ProductionExecutor<'a> {
    pub fn new(runner: &'a ProcessRunner, artifacts: &'a ArtifactStore) -> Self {
        Self { runner, artifacts }
    }
}

impl ProcessExecutor for ProductionExecutor<'_> {
    fn execute(
        &mut self,
        request: &CommandRequest,
        targets: &ArtifactTargets,
    ) -> Result<(CommandResult, StoredOutput), ProcessError> {
        let result = self.runner.run(request)?;
        let stored = self
            .runner
            .store_output(self.artifacts, targets, &result, |_| Ok::<_, ()>(()))?;
        Ok((result, stored))
    }
}

pub struct Verifier<'a, E> {
    contract: &'a TaskContract,
    status: TaskStatus,
    repository: PathBuf,
    executor: E,
}

impl<'a, E: ProcessExecutor> Verifier<'a, E> {
    pub fn new(
        contract: &'a TaskContract,
        status: TaskStatus,
        repository: impl AsRef<Path>,
        executor: E,
    ) -> Result<Self, VerifierError> {
        if contract.verification_commands.is_empty() {
            return Err(VerifierError::EmptyVerificationList);
        }
        if contract.verification_commands.len() > MAX_COMMANDS {
            return Err(VerifierError::InvalidCommand);
        }
        let repository = repository
            .as_ref()
            .canonicalize()
            .map_err(|_| VerifierError::GitInspectionFailed)?;
        if !repository.is_dir() {
            return Err(VerifierError::GitInspectionFailed);
        }
        Ok(Self {
            contract,
            status,
            repository,
            executor,
        })
    }

    pub fn verify(mut self) -> Result<VerificationReport, VerifierError> {
        let baseline = git_snapshot(&self.repository)?;
        let mut results = Vec::new();
        let mut verdict = VerificationVerdict::Integrate;
        for (index, raw) in self.contract.verification_commands.iter().enumerate() {
            let command = parse_command(raw.as_str())?;
            let targets = artifact_targets(self.contract, index, raw.as_str());
            let execution = self.executor.execute(
                &CommandRequest {
                    command,
                    working_directory: String::new(),
                    environment_keys: Vec::new(),
                },
                &targets,
            );
            if git_snapshot(&self.repository)? != baseline {
                return Err(VerifierError::SourceMutation);
            }
            let (result, stored) = execution.map_err(map_process_error)?;
            let passed = !result.audit.timed_out && result.audit.exit_code == Some(0);
            results.push(VerificationResult {
                command: raw.as_str().to_owned(),
                exit_code: result.audit.exit_code,
                duration_ms: result.audit.duration_ms,
                timed_out: result.audit.timed_out,
                stdout_artifact_id: stored.stdout.artifact_id,
                stderr_artifact_id: stored.stderr.artifact_id,
                passed,
            });
            if !passed {
                verdict = VerificationVerdict::Failed;
                break;
            }
        }
        let proposed_status = match verdict {
            VerificationVerdict::Integrate => TaskStatus::Integrate,
            VerificationVerdict::Failed => TaskStatus::Failed,
        };
        transition(self.contract, self.status, proposed_status, Actor::Verifier)
            .map_err(|_| VerifierError::InvalidTransition)?;
        Ok(VerificationReport {
            verdict,
            proposed_status,
            results,
        })
    }
}

fn parse_command(value: &str) -> Result<VerificationCommand, VerifierError> {
    if value.is_empty()
        || value.len() > MAX_COMMAND_BYTES
        || value.chars().any(char::is_control)
        || value.contains([';', '|', '&', '`', '$', '<', '>', '\'', '"', '\\'])
    {
        return Err(VerifierError::InvalidCommand);
    }
    let mut parts = value.split_ascii_whitespace();
    let executable = parts.next().ok_or(VerifierError::InvalidCommand)?;
    let arguments: Vec<_> = parts.map(str::to_owned).collect();
    if executable.starts_with('-')
        || executable.contains('/')
        || arguments.len() > 64
        || arguments.iter().any(|argument| argument.len() > 1_024)
    {
        return Err(VerifierError::InvalidCommand);
    }
    Ok(VerificationCommand {
        class: "verification".to_owned(),
        executable: executable.to_owned(),
        arguments,
    })
}

fn artifact_targets(contract: &TaskContract, index: usize, command: &str) -> ArtifactTargets {
    let mut digest = Sha256::new();
    digest.update(contract.project_run_id.as_str());
    digest.update([0]);
    digest.update(contract.id.as_str());
    digest.update([0]);
    digest.update(index.to_le_bytes());
    digest.update(command);
    let digest = format!("{:x}", digest.finalize());
    ArtifactTargets {
        stdout_id: format!("verify-{}-stdout", &digest[..24]),
        stderr_id: format!("verify-{}-stderr", &digest[..24]),
    }
}

fn git_snapshot(repository: &Path) -> Result<[u8; 32], VerifierError> {
    let mut digest = Sha256::new();
    hash_git_output(
        repository,
        ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        &mut digest,
    )?;
    hash_git_output(
        repository,
        ["diff", "--binary", "--no-ext-diff", "HEAD", "--"],
        &mut digest,
    )?;
    let untracked = git_output(
        repository,
        ["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    let mut total =
        u64::try_from(untracked.len()).map_err(|_| VerifierError::GitInspectionFailed)?;
    digest.update(&untracked);
    for raw in untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let relative = std::str::from_utf8(raw).map_err(|_| VerifierError::GitInspectionFailed)?;
        let relative = Path::new(relative);
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(VerifierError::GitInspectionFailed);
        }
        let path = repository.join(relative);
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| VerifierError::GitInspectionFailed)?;
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(path).map_err(|_| VerifierError::GitInspectionFailed)?;
            let target = target.to_str().ok_or(VerifierError::GitInspectionFailed)?;
            add_snapshot_bytes(&mut digest, target.as_bytes(), &mut total)?;
        } else if metadata.is_file() {
            let file = fs::File::open(path).map_err(|_| VerifierError::GitInspectionFailed)?;
            hash_reader(file, &mut digest, &mut total)?;
        }
    }
    Ok(digest.finalize().into())
}

fn hash_git_output<const N: usize>(
    repository: &Path,
    arguments: [&str; N],
    digest: &mut Sha256,
) -> Result<(), VerifierError> {
    let output = git_output(repository, arguments)?;
    let mut total = 0;
    add_snapshot_bytes(digest, &output, &mut total)
}

fn git_output<const N: usize>(
    repository: &Path,
    arguments: [&str; N],
) -> Result<Vec<u8>, VerifierError> {
    let mut child = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| VerifierError::GitInspectionFailed)?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or(VerifierError::GitInspectionFailed)?;
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8 * 1024];
    let mut total = 0_u64;
    loop {
        let read = stdout
            .read(&mut buffer)
            .map_err(|_| VerifierError::GitInspectionFailed)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > MAX_GIT_SNAPSHOT_BYTES {
            let _ = child.kill();
            let _ = child.wait();
            return Err(VerifierError::GitInspectionFailed);
        }
        output
            .write_all(&buffer[..read])
            .map_err(|_| VerifierError::GitInspectionFailed)?;
    }
    let status = child
        .wait()
        .map_err(|_| VerifierError::GitInspectionFailed)?;
    if !status.success() {
        return Err(VerifierError::GitInspectionFailed);
    }
    Ok(output)
}

fn hash_reader(
    mut reader: impl Read,
    digest: &mut Sha256,
    total: &mut u64,
) -> Result<(), VerifierError> {
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| VerifierError::GitInspectionFailed)?;
        if read == 0 {
            return Ok(());
        }
        add_snapshot_bytes(digest, &buffer[..read], total)?;
    }
}

fn add_snapshot_bytes(
    digest: &mut Sha256,
    bytes: &[u8],
    total: &mut u64,
) -> Result<(), VerifierError> {
    *total = total.saturating_add(bytes.len() as u64);
    if *total > MAX_GIT_SNAPSHOT_BYTES {
        return Err(VerifierError::GitInspectionFailed);
    }
    digest.update(bytes);
    Ok(())
}

fn map_process_error(error: ProcessError) -> VerifierError {
    match error {
        ProcessError::CommandDenied | ProcessError::InvalidCommand => VerifierError::CommandDenied,
        ProcessError::ArtifactFailed => VerifierError::ArtifactFailed,
        _ => VerifierError::ExecutionFailed,
    }
}
