use std::{
    collections::BTreeSet,
    fmt,
    io::Read,
    path::{Component, Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use crate::store::artifact::{ArtifactError, ArtifactMetadata, ArtifactStore};

use super::container::{ContainerError, ContainerLimits, ContainerSpec, CreatedContainer};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationCommand {
    pub class: String,
    pub executable: String,
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandRequest {
    pub command: VerificationCommand,
    pub working_directory: String,
    pub environment_keys: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionAudit {
    pub command_class: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub audit: ExecutionAudit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredOutput {
    pub stdout: ArtifactMetadata,
    pub stderr: ArtifactMetadata,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactTargets {
    pub stdout_id: String,
    pub stderr_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessError {
    InvalidConfiguration,
    InvalidCommand,
    CommandDenied,
    InvalidWorkingDirectory,
    EnvironmentDenied,
    ContainerUnavailable,
    ContainerFailed,
    OutputCaptureFailed,
    ArtifactFailed,
}

impl fmt::Display for ProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "invalid process runner configuration",
            Self::InvalidCommand => "invalid verification command",
            Self::CommandDenied => "verification command denied",
            Self::InvalidWorkingDirectory => "invalid container working directory",
            Self::EnvironmentDenied => "environment key denied",
            Self::ContainerUnavailable => "container runtime unavailable",
            Self::ContainerFailed => "container execution failed",
            Self::OutputCaptureFailed => "container output capture failed",
            Self::ArtifactFailed => "execution artifact write failed",
        })
    }
}

impl std::error::Error for ProcessError {}

pub struct ProcessRunner {
    worktree: PathBuf,
    image: String,
    allowed_commands: Vec<VerificationCommand>,
    allowed_environment: BTreeSet<String>,
    timeout: Duration,
    maximum_output_bytes: usize,
    limits: ContainerLimits,
}

impl ProcessRunner {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        worktree: impl AsRef<Path>,
        image: impl Into<String>,
        allowed_commands: Vec<VerificationCommand>,
        allowed_environment: Vec<String>,
        timeout: Duration,
        maximum_output_bytes: usize,
        limits: ContainerLimits,
    ) -> Result<Self, ProcessError> {
        let worktree = worktree
            .as_ref()
            .canonicalize()
            .map_err(|_| ProcessError::InvalidConfiguration)?;
        let image = image.into();
        if !worktree.is_dir()
            || image.is_empty()
            || image.starts_with('-')
            || image.chars().any(char::is_control)
            || allowed_commands.is_empty()
            || timeout.is_zero()
            || maximum_output_bytes == 0
            || limits.memory_bytes == 0
            || limits.process_limit == 0
            || limits
                .cpu_count
                .parse::<f64>()
                .ok()
                .is_none_or(|cpu| cpu <= 0.0)
        {
            return Err(ProcessError::InvalidConfiguration);
        }
        for command in &allowed_commands {
            validate_command(command)?;
        }
        let mut environment = BTreeSet::new();
        for key in allowed_environment {
            validate_environment_key(&key)?;
            environment.insert(key);
        }
        Ok(Self {
            worktree,
            image,
            allowed_commands,
            allowed_environment: environment,
            timeout,
            maximum_output_bytes,
            limits,
        })
    }

    pub fn run(&self, request: &CommandRequest) -> Result<CommandResult, ProcessError> {
        validate_command(&request.command)?;
        if !self.allowed_commands.contains(&request.command) {
            return Err(ProcessError::CommandDenied);
        }
        validate_working_directory(&self.worktree, &request.working_directory)?;
        let mut environment = request.environment_keys.clone();
        environment.sort();
        environment.dedup();
        for key in &environment {
            validate_environment_key(key)?;
            if !self.allowed_environment.contains(key) {
                return Err(ProcessError::EnvironmentDenied);
            }
        }
        let container = CreatedContainer::create(&ContainerSpec {
            image: &self.image,
            worktree: &self.worktree,
            working_directory: &request.working_directory,
            executable: &request.command.executable,
            arguments: &request.command.arguments,
            environment_keys: &environment,
            limits: &self.limits,
        })
        .map_err(map_container_error)?;
        let started = Instant::now();
        let mut child = container
            .start_command()
            .spawn()
            .map_err(|_| ProcessError::ContainerUnavailable)?;
        let stdout = child
            .stdout
            .take()
            .ok_or(ProcessError::OutputCaptureFailed)?;
        let stderr = child
            .stderr
            .take()
            .ok_or(ProcessError::OutputCaptureFailed)?;
        let stdout_reader = bounded_reader(stdout, self.maximum_output_bytes);
        let stderr_reader = bounded_reader(stderr, self.maximum_output_bytes);
        let timed_out = loop {
            if child
                .try_wait()
                .map_err(|_| ProcessError::ContainerFailed)?
                .is_some()
            {
                break false;
            }
            if started.elapsed() >= self.timeout {
                container.kill();
                let _ = child.kill();
                let _ = child.wait();
                break true;
            }
            thread::sleep(Duration::from_millis(10));
        };
        let (stdout, stdout_truncated) = stdout_reader
            .join()
            .map_err(|_| ProcessError::OutputCaptureFailed)??;
        let (stderr, stderr_truncated) = stderr_reader
            .join()
            .map_err(|_| ProcessError::OutputCaptureFailed)??;
        let exit_code = if timed_out {
            None
        } else {
            Some(container.inspect_exit_code().map_err(map_container_error)?)
        };
        container.cleanup().map_err(map_container_error)?;
        Ok(CommandResult {
            stdout,
            stderr,
            audit: ExecutionAudit {
                command_class: request.command.class.clone(),
                exit_code,
                timed_out,
                duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                stdout_truncated,
                stderr_truncated,
            },
        })
    }

    pub fn store_output<F, E>(
        &self,
        store: &ArtifactStore,
        targets: &ArtifactTargets,
        result: &CommandResult,
        persist_metadata: F,
    ) -> Result<StoredOutput, ProcessError>
    where
        F: Fn(&ArtifactMetadata) -> Result<(), E>,
    {
        let stdout = store
            .write(
                &targets.stdout_id,
                "stdout.txt",
                "text/plain",
                &result.stdout,
                &persist_metadata,
            )
            .map_err(map_artifact_error)?;
        let stderr = store
            .write(
                &targets.stderr_id,
                "stderr.txt",
                "text/plain",
                &result.stderr,
                persist_metadata,
            )
            .map_err(map_artifact_error)?;
        Ok(StoredOutput { stdout, stderr })
    }
}

fn bounded_reader<R: Read + Send + 'static>(
    mut reader: R,
    maximum: usize,
) -> thread::JoinHandle<Result<(Vec<u8>, bool), ProcessError>> {
    thread::spawn(move || {
        let mut output = Vec::with_capacity(maximum.min(8 * 1024));
        let mut buffer = [0_u8; 8 * 1024];
        let mut truncated = false;
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|_| ProcessError::OutputCaptureFailed)?;
            if read == 0 {
                break;
            }
            let remaining = maximum.saturating_sub(output.len());
            output.extend_from_slice(&buffer[..read.min(remaining)]);
            truncated |= read > remaining;
        }
        Ok((output, truncated))
    })
}

fn validate_command(command: &VerificationCommand) -> Result<(), ProcessError> {
    if command.class.is_empty()
        || command.class.len() > 64
        || invalid_text(&command.class)
        || command.executable.is_empty()
        || command.executable.len() > 128
        || command.executable.starts_with('-')
        || command.executable.contains('/')
        || command.executable.contains('\\')
        || invalid_text(&command.executable)
        || matches!(command.executable.as_str(), "sh" | "bash" | "dash" | "zsh")
        || command.arguments.len() > 64
        || command.arguments.iter().any(|argument| {
            argument.len() > 1_024 || invalid_text(argument) || contains_shell_operator(argument)
        })
    {
        return Err(ProcessError::InvalidCommand);
    }
    Ok(())
}

fn invalid_text(value: &str) -> bool {
    value.chars().any(char::is_control)
}

fn contains_shell_operator(value: &str) -> bool {
    value.contains([';', '|', '&', '`', '$', '<', '>'])
}

fn validate_environment_key(key: &str) -> Result<(), ProcessError> {
    let mut characters = key.chars();
    if key.len() > 128
        || !characters
            .next()
            .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        || !characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err(ProcessError::EnvironmentDenied);
    }
    Ok(())
}

fn validate_working_directory(root: &Path, relative: &str) -> Result<(), ProcessError> {
    if relative.chars().any(char::is_control)
        || relative.contains('\\')
        || Path::new(relative).is_absolute()
        || Path::new(relative)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
            && !relative.is_empty()
    {
        return Err(ProcessError::InvalidWorkingDirectory);
    }
    let path = root.join(relative);
    let canonical = path
        .canonicalize()
        .map_err(|_| ProcessError::InvalidWorkingDirectory)?;
    if !canonical.starts_with(root) || !canonical.is_dir() {
        return Err(ProcessError::InvalidWorkingDirectory);
    }
    Ok(())
}

fn map_container_error(error: ContainerError) -> ProcessError {
    match error {
        ContainerError::DockerUnavailable => ProcessError::ContainerUnavailable,
        ContainerError::CreateFailed
        | ContainerError::StartFailed
        | ContainerError::CleanupFailed => ProcessError::ContainerFailed,
    }
}

fn map_artifact_error(_: ArtifactError) -> ProcessError {
    ProcessError::ArtifactFailed
}
