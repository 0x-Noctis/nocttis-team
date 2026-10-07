use std::{
    fs,
    path::PathBuf,
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use ai_team::{
    runner::{
        container::{ContainerLimits, docker_available},
        process::{
            ArtifactTargets, CommandRequest, ProcessError, ProcessRunner, VerificationCommand,
        },
    },
    store::artifact::ArtifactStore,
};

static DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static DOCKER_TEST: Mutex<()> = Mutex::new(());
const IMAGE: &str = "alpine:latest";

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let sequence = DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "noctis-process-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn command(class: &str, executable: &str, arguments: &[&str]) -> VerificationCommand {
    VerificationCommand {
        class: class.into(),
        executable: executable.into(),
        arguments: arguments.iter().map(|value| (*value).into()).collect(),
    }
}

fn limits() -> ContainerLimits {
    ContainerLimits {
        cpu_count: "0.5".into(),
        memory_bytes: 64 * 1024 * 1024,
        process_limit: 32,
    }
}

fn runner(
    worktree: &TestDirectory,
    allowed: Vec<VerificationCommand>,
    environment: Vec<String>,
    timeout: Duration,
    output: usize,
) -> ProcessRunner {
    ProcessRunner::new(
        &worktree.0,
        IMAGE,
        allowed,
        environment,
        timeout,
        output,
        limits(),
    )
    .unwrap()
}

fn request(command: VerificationCommand) -> CommandRequest {
    CommandRequest {
        command,
        working_directory: String::new(),
        environment_keys: Vec::new(),
    }
}

fn docker_ready() -> bool {
    if docker_available().is_err() {
        eprintln!("skipped: Docker daemon unavailable");
        return false;
    }
    let available = std::process::Command::new("docker")
        .args(["image", "inspect", IMAGE])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !available {
        eprintln!("skipped: local {IMAGE} image unavailable; test does not pull over network");
    }
    available
}

fn docker_test() -> Option<MutexGuard<'static, ()>> {
    let guard = DOCKER_TEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !docker_ready() {
        return None;
    }
    remove_runner_containers();
    Some(guard)
}

#[test]
fn policy_rejects_arbitrary_absolute_and_shell_commands() {
    let worktree = TestDirectory::new("policy");
    let allowed = command("verify", "printf", &["ok"]);
    let runner = runner(
        &worktree,
        vec![allowed.clone()],
        Vec::new(),
        Duration::from_secs(1),
        100,
    );

    assert_eq!(
        runner.run(&request(command("verify", "rm", &["-rf", "."]))),
        Err(ProcessError::CommandDenied)
    );
    assert_eq!(
        runner.run(&request(command("verify", "/bin/printf", &["ok"]))),
        Err(ProcessError::InvalidCommand)
    );
    assert_eq!(
        runner.run(&request(command("verify", "printf", &["ok; touch owned"]))),
        Err(ProcessError::InvalidCommand)
    );
    assert_eq!(
        runner.run(&request(command("verify", "printf", &["ok\nowned"]))),
        Err(ProcessError::InvalidCommand)
    );
    assert!(!worktree.0.join("owned").exists());
}

#[test]
fn policy_rejects_non_allowlisted_environment_and_unsafe_workdir() {
    let worktree = TestDirectory::new("env-policy");
    let allowed = command("environment", "env", &[]);
    let runner = runner(
        &worktree,
        vec![allowed.clone()],
        vec!["SAFE_VALUE".into()],
        Duration::from_secs(1),
        100,
    );
    let mut denied = request(allowed.clone());
    denied.environment_keys = vec!["SECRET_MARKER".into()];
    assert_eq!(runner.run(&denied), Err(ProcessError::EnvironmentDenied));
    let mut traversal = request(allowed);
    traversal.working_directory = "../".into();
    assert_eq!(
        runner.run(&traversal),
        Err(ProcessError::InvalidWorkingDirectory)
    );
}

#[test]
fn allowed_command_runs_and_output_can_be_stored() {
    let Some(_docker) = docker_test() else {
        return;
    };
    let worktree = TestDirectory::new("success");
    let artifacts = TestDirectory::new("success-artifacts");
    let allowed = command("verification", "printf", &["verified"]);
    let runner = runner(
        &worktree,
        vec![allowed.clone()],
        Vec::new(),
        Duration::from_secs(5),
        100,
    );

    let result = runner.run(&request(allowed)).unwrap();
    assert_eq!(result.stdout, b"verified");
    assert_eq!(result.audit.exit_code, Some(0));
    assert!(!result.audit.timed_out);
    assert!(!result.audit.stdout_truncated);
    assert_eq!(result.audit.command_class, "verification");
    let store = ArtifactStore::new(&artifacts.0, 1_000).unwrap();
    let stored = runner
        .store_output(
            &store,
            &ArtifactTargets {
                stdout_id: "stdout-1".into(),
                stderr_id: "stderr-1".into(),
            },
            &result,
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    assert_eq!(stored.stdout.size, 8);
    assert_eq!(store.read("stdout-1").unwrap(), b"verified");
}

#[test]
fn timeout_kills_container_process_tree_and_cleans_up() {
    let Some(_docker) = docker_test() else {
        return;
    };
    let worktree = TestDirectory::new("timeout");
    let allowed = command("timeout", "sleep", &["30"]);
    let runner = runner(
        &worktree,
        vec![allowed.clone()],
        Vec::new(),
        Duration::from_millis(150),
        100,
    );
    let started = Instant::now();

    let result = runner.run(&request(allowed)).unwrap();
    assert!(result.audit.timed_out);
    assert_eq!(result.audit.exit_code, None);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_no_runner_containers();
}

#[test]
fn oversized_stdout_and_stderr_are_bounded() {
    let Some(_docker) = docker_test() else {
        return;
    };
    let worktree = TestDirectory::new("output");
    let stdout = command("stdout", "yes", &["out"]);
    let stderr = command(
        "stderr",
        "ls",
        &[
            "missing-01",
            "missing-02",
            "missing-03",
            "missing-04",
            "missing-05",
            "missing-06",
            "missing-07",
            "missing-08",
            "missing-09",
            "missing-10",
        ],
    );
    let runner = runner(
        &worktree,
        vec![stdout.clone(), stderr.clone()],
        Vec::new(),
        Duration::from_millis(150),
        128,
    );

    let stdout_result = runner.run(&request(stdout)).unwrap();
    assert_eq!(stdout_result.stdout.len(), 128);
    assert!(stdout_result.audit.stdout_truncated);
    let stderr_result = runner.run(&request(stderr)).unwrap();
    assert_eq!(stderr_result.stderr.len(), 128);
    assert!(stderr_result.audit.stderr_truncated);
    assert_no_runner_containers();
}

#[test]
fn host_environment_is_not_forwarded_without_allowlist() {
    let Some(_docker) = docker_test() else {
        return;
    };
    let worktree = TestDirectory::new("environment");
    let allowed = command("environment", "env", &[]);
    let runner = runner(
        &worktree,
        vec![allowed.clone()],
        Vec::new(),
        Duration::from_secs(5),
        4_000,
    );
    unsafe { std::env::set_var("SECRET_MARKER", "do-not-forward") };
    let result = runner.run(&request(allowed)).unwrap();
    unsafe { std::env::remove_var("SECRET_MARKER") };

    assert!(!String::from_utf8_lossy(&result.stdout).contains("do-not-forward"));
}

#[test]
fn allowlisted_environment_key_is_forwarded_without_entering_audit() {
    let Some(_docker) = docker_test() else {
        return;
    };
    let worktree = TestDirectory::new("allowed-environment");
    let allowed = command("environment", "env", &[]);
    let runner = runner(
        &worktree,
        vec![allowed.clone()],
        vec!["SAFE_VALUE".into()],
        Duration::from_secs(5),
        4_000,
    );
    unsafe { std::env::set_var("SAFE_VALUE", "forwarded-value") };
    let mut request = request(allowed);
    request.environment_keys = vec!["SAFE_VALUE".into()];
    let result = runner.run(&request).unwrap();
    unsafe { std::env::remove_var("SAFE_VALUE") };

    assert!(String::from_utf8_lossy(&result.stdout).contains("SAFE_VALUE=forwarded-value"));
    assert!(!format!("{:?}", result.audit).contains("forwarded-value"));
}

#[test]
fn network_and_root_filesystem_are_disabled_and_limits_apply() {
    let Some(_docker) = docker_test() else {
        return;
    };
    let worktree = TestDirectory::new("isolation");
    let network = command("network", "wget", &["-T", "1", "http://example.com"]);
    let filesystem = command("filesystem", "touch", &["/forbidden"]);
    let worktree_write = command("worktree", "touch", &["generated.txt"]);
    let resources = command(
        "resources",
        "cat",
        &[
            "/sys/fs/cgroup/pids.max",
            "/sys/fs/cgroup/memory.max",
            "/sys/fs/cgroup/cpu.max",
        ],
    );
    let runner = runner(
        &worktree,
        vec![
            network.clone(),
            filesystem.clone(),
            worktree_write.clone(),
            resources.clone(),
        ],
        Vec::new(),
        Duration::from_secs(5),
        4_000,
    );

    assert_ne!(
        runner.run(&request(network)).unwrap().audit.exit_code,
        Some(0)
    );
    assert_ne!(
        runner.run(&request(filesystem)).unwrap().audit.exit_code,
        Some(0)
    );
    assert_eq!(
        runner
            .run(&request(worktree_write))
            .unwrap()
            .audit
            .exit_code,
        Some(0)
    );
    assert!(worktree.0.join("generated.txt").exists());
    let result = runner.run(&request(resources)).unwrap();
    let output = String::from_utf8(result.stdout).unwrap();
    let lines: Vec<_> = output.lines().collect();
    assert_eq!(lines[0], "32");
    assert_eq!(lines[1], (64 * 1024 * 1024).to_string());
    assert!(lines[2].starts_with("50000 "));
    assert_no_runner_containers();
}

#[test]
fn errors_and_audit_do_not_expose_host_path_or_secret() {
    let worktree = TestDirectory::new("redaction");
    let allowed = command("verification", "printf", &["ok"]);
    let runner = runner(
        &worktree,
        vec![allowed.clone()],
        Vec::new(),
        Duration::from_secs(1),
        100,
    );
    let mut denied = request(allowed);
    denied.environment_keys = vec!["SECRET_MARKER".into()];
    let error = runner.run(&denied).unwrap_err().to_string();

    assert!(!error.contains(worktree.0.to_string_lossy().as_ref()));
    assert!(!error.contains("SECRET_MARKER"));
}

fn assert_no_runner_containers() {
    let output = std::process::Command::new("docker")
        .args([
            "ps",
            "--all",
            "--filter",
            &format!("name=noctis-runner-{}-", std::process::id()),
            "--format",
            "{{.Names}}",
        ])
        .output()
        .unwrap();
    assert!(output.stdout.is_empty());
}

fn remove_runner_containers() {
    let output = std::process::Command::new("docker")
        .args([
            "ps",
            "--all",
            "--filter",
            &format!("name=noctis-runner-{}-", std::process::id()),
            "--format",
            "{{.Names}}",
        ])
        .output()
        .unwrap();
    for name in String::from_utf8(output.stdout).unwrap().lines() {
        let _ = std::process::Command::new("docker")
            .args(["rm", "--force", name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[test]
fn command_output_is_redacted_before_it_leaves_the_runner() {
    let Some(_docker) = docker_test() else {
        return;
    };
    let worktree = TestDirectory::new("redaction");
    let secret = command(
        "secret",
        "printf",
        &["API_KEY=sk-abcdefghijklmnopqrstuvwxyz012345 visible"],
    );
    let runner = runner(
        &worktree,
        vec![secret.clone()],
        Vec::new(),
        Duration::from_secs(5),
        4_000,
    );
    let result = runner.run(&request(secret)).unwrap();
    let stdout = String::from_utf8(result.stdout).unwrap();
    assert_eq!(result.audit.exit_code, Some(0));
    assert!(!stdout.contains("sk-abcdefghijkl"), "{stdout}");
    assert!(
        stdout.contains("visible") && stdout.contains("[REDACTED]"),
        "{stdout}"
    );
}
