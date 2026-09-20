use std::{
    fmt,
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

static CONTAINER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainerLimits {
    pub cpu_count: String,
    pub memory_bytes: u64,
    pub process_limit: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainerError {
    DockerUnavailable,
    CreateFailed,
    StartFailed,
    CleanupFailed,
}

impl fmt::Display for ContainerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DockerUnavailable => "container runtime unavailable",
            Self::CreateFailed => "container creation failed",
            Self::StartFailed => "container execution failed",
            Self::CleanupFailed => "container cleanup failed",
        })
    }
}

impl std::error::Error for ContainerError {}

pub struct ContainerSpec<'a> {
    pub image: &'a str,
    pub worktree: &'a Path,
    pub working_directory: &'a str,
    pub executable: &'a str,
    pub arguments: &'a [String],
    pub environment_keys: &'a [String],
    pub limits: &'a ContainerLimits,
}

pub struct CreatedContainer {
    name: String,
    cleaned: bool,
}

impl CreatedContainer {
    pub fn create(spec: &ContainerSpec<'_>) -> Result<Self, ContainerError> {
        docker_available()?;
        let name = format!(
            "noctis-runner-{}-{}",
            std::process::id(),
            CONTAINER_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let working_directory = if spec.working_directory.is_empty() {
            "/workspace".to_owned()
        } else {
            format!("/workspace/{}", spec.working_directory)
        };
        let mut command = Command::new("docker");
        command
            .arg("create")
            .arg("--name")
            .arg(&name)
            .arg("--network")
            .arg("none")
            .arg("--read-only")
            .arg("--cap-drop")
            .arg("ALL")
            .arg("--cap-add")
            .arg("DAC_OVERRIDE")
            .arg("--security-opt")
            .arg("no-new-privileges")
            .arg("--cpus")
            .arg(&spec.limits.cpu_count)
            .arg("--memory")
            .arg(spec.limits.memory_bytes.to_string())
            .arg("--pids-limit")
            .arg(spec.limits.process_limit.to_string())
            .arg("--mount")
            .arg(format!(
                "type=bind,src={},dst=/workspace",
                spec.worktree.display()
            ))
            .arg("--workdir")
            .arg(working_directory);
        for key in spec.environment_keys {
            command.arg("--env").arg(key);
        }
        let status = command
            .arg(spec.image)
            .arg(spec.executable)
            .args(spec.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|_| ContainerError::DockerUnavailable)?;
        if !status.success() {
            let _ = remove(&name);
            return Err(ContainerError::CreateFailed);
        }
        Ok(Self {
            name,
            cleaned: false,
        })
    }

    pub fn start_command(&self) -> Command {
        let mut command = Command::new("docker");
        command
            .arg("start")
            .arg("--attach")
            .arg(&self.name)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    pub fn inspect_exit_code(&self) -> Result<i32, ContainerError> {
        let output = Command::new("docker")
            .args(["inspect", "--format", "{{.State.ExitCode}}", &self.name])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|_| ContainerError::DockerUnavailable)?;
        if !output.status.success() {
            return Err(ContainerError::StartFailed);
        }
        std::str::from_utf8(&output.stdout)
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .ok_or(ContainerError::StartFailed)
    }

    pub fn kill(&self) {
        let _ = Command::new("docker")
            .args(["kill", &self.name])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    pub fn cleanup(mut self) -> Result<(), ContainerError> {
        remove(&self.name)?;
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for CreatedContainer {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = remove(&self.name);
        }
    }
}

pub fn docker_available() -> Result<(), ContainerError> {
    let status = Command::new("docker")
        .arg("version")
        .arg("--format")
        .arg("{{.Server.Version}}")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| ContainerError::DockerUnavailable)?;
    status
        .success()
        .then_some(())
        .ok_or(ContainerError::DockerUnavailable)
}

fn remove(name: &str) -> Result<(), ContainerError> {
    let status = Command::new("docker")
        .args(["rm", "--force", name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| ContainerError::DockerUnavailable)?;
    status
        .success()
        .then_some(())
        .ok_or(ContainerError::CleanupFailed)
}
