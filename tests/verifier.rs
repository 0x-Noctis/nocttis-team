use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use ai_team::{
    agent::verifier::{ProcessExecutor, VerificationVerdict, Verifier, VerifierError},
    domain::task::{
        AllowedPath, MaxAttempts, NonEmptyString, PositiveLimit, TaskContract, TaskLimits,
        TaskStatus,
    },
    runner::process::{
        ArtifactTargets, CommandRequest, CommandResult, ExecutionAudit, ProcessError, StoredOutput,
    },
    store::artifact::ArtifactMetadata,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestRepository(PathBuf);

impl TestRepository {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "noctis-verifier-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        git(&path, &["init", "--quiet"]);
        git(&path, &["config", "user.email", "test@example.invalid"]);
        git(&path, &["config", "user.name", "Test"]);
        fs::write(path.join("source.txt"), "baseline\n").unwrap();
        git(&path, &["add", "source.txt"]);
        git(&path, &["commit", "--quiet", "-m", "fixture"]);
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestRepository {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn git(repository: &Path, arguments: &[&str]) {
    assert!(
        Command::new("git")
            .args(arguments)
            .current_dir(repository)
            .status()
            .unwrap()
            .success()
    );
}

#[derive(Clone)]
struct PlannedExecution {
    result: Result<(CommandResult, StoredOutput), ProcessError>,
    mutation: Option<(PathBuf, Vec<u8>)>,
}

struct FakeExecutor {
    planned: VecDeque<PlannedExecution>,
    commands: Vec<CommandRequest>,
    targets: Vec<ArtifactTargets>,
}

impl FakeExecutor {
    fn new(planned: Vec<PlannedExecution>) -> Self {
        Self {
            planned: planned.into(),
            commands: Vec::new(),
            targets: Vec::new(),
        }
    }
}

impl ProcessExecutor for &mut FakeExecutor {
    fn execute(
        &mut self,
        request: &CommandRequest,
        targets: &ArtifactTargets,
    ) -> Result<(CommandResult, StoredOutput), ProcessError> {
        self.commands.push(request.clone());
        self.targets.push(targets.clone());
        let planned = self.planned.pop_front().expect("unexpected execution");
        if let Some((path, bytes)) = planned.mutation {
            fs::write(path, bytes).unwrap();
        }
        planned.result
    }
}

fn contract(commands: &[&str]) -> TaskContract {
    TaskContract {
        id: text("id", "M2-012"),
        project_id: text("project_id", "project"),
        project_run_id: text("project_run_id", "run"),
        title: text("title", "Verifier"),
        role: text("role", "Execution Plane Engineer"),
        objective: text("objective", "Verify deterministically"),
        depends_on: Vec::new(),
        allowed_paths: vec![AllowedPath::parse("src/**").unwrap()],
        context_refs: Vec::new(),
        acceptance_criteria: vec![text("acceptance_criteria", "Commands pass")],
        verification_commands: commands
            .iter()
            .map(|command| text("verification_commands", command))
            .collect(),
        limits: TaskLimits {
            max_input_tokens: PositiveLimit::new("max_input_tokens", 100).unwrap(),
            max_output_tokens: PositiveLimit::new("max_output_tokens", 100).unwrap(),
            max_tool_calls: PositiveLimit::new("max_tool_calls", 10).unwrap(),
            max_attempts: MaxAttempts::new(2).unwrap(),
            timeout_seconds: PositiveLimit::new("timeout_seconds", 60).unwrap(),
        },
    }
}

fn text(field: &'static str, value: &str) -> NonEmptyString {
    NonEmptyString::parse(field, value).unwrap()
}

fn execution(exit_code: Option<i32>, timed_out: bool, suffix: &str) -> PlannedExecution {
    let stdout_id = format!("stdout-{suffix}");
    let stderr_id = format!("stderr-{suffix}");
    PlannedExecution {
        result: Ok((
            CommandResult {
                stdout: vec![b'x'; 32],
                stderr: vec![b'y'; 32],
                audit: ExecutionAudit {
                    command_class: "verification".to_owned(),
                    exit_code,
                    timed_out,
                    duration_ms: 7,
                    stdout_truncated: true,
                    stderr_truncated: true,
                },
            },
            StoredOutput {
                stdout: metadata(&stdout_id),
                stderr: metadata(&stderr_id),
            },
        )),
        mutation: None,
    }
}

fn metadata(id: &str) -> ArtifactMetadata {
    ArtifactMetadata {
        artifact_id: id.to_owned(),
        logical_name: "evidence.txt".to_owned(),
        media_type: "text/plain".to_owned(),
        size: 32,
        checksum: "0".repeat(64),
    }
}

#[test]
fn runs_commands_in_order_and_proposes_integrate() {
    let repository = TestRepository::new();
    let task = contract(&["cargo test", "cargo clippy --all-targets"]);
    let mut executor = FakeExecutor::new(vec![
        execution(Some(0), false, "1"),
        execution(Some(0), false, "2"),
    ]);

    let report = Verifier::new(&task, TaskStatus::Verify, repository.path(), &mut executor)
        .unwrap()
        .verify()
        .unwrap();

    assert_eq!(report.verdict, VerificationVerdict::Integrate);
    assert_eq!(report.proposed_status, TaskStatus::Integrate);
    assert_eq!(report.results.len(), 2);
    assert!(report.results.iter().all(|result| result.passed));
    assert_eq!(executor.commands[0].command.executable, "cargo");
    assert_eq!(executor.commands[0].command.arguments, ["test"]);
    assert_eq!(
        executor.commands[1].command.arguments,
        ["clippy", "--all-targets"]
    );
    assert_eq!(report.results[0].stdout_artifact_id, "stdout-1");
}

#[test]
fn nonzero_exit_fails_and_stops() {
    let repository = TestRepository::new();
    let task = contract(&["cargo test", "cargo clippy"]);
    let mut executor = FakeExecutor::new(vec![
        execution(Some(2), false, "failed"),
        execution(Some(0), false, "unused"),
    ]);

    let report = Verifier::new(&task, TaskStatus::Verify, repository.path(), &mut executor)
        .unwrap()
        .verify()
        .unwrap();

    assert_eq!(report.verdict, VerificationVerdict::Failed);
    assert_eq!(report.proposed_status, TaskStatus::Failed);
    assert_eq!(report.results.len(), 1);
    assert!(!report.results[0].passed);
    assert_eq!(executor.commands.len(), 1);
}

#[test]
fn timeout_fails_from_audit_not_output_text() {
    let repository = TestRepository::new();
    let task = contract(&["cargo test"]);
    let mut timeout = execution(None, true, "timeout");
    timeout.result.as_mut().unwrap().0.stdout = b"all tests passed".to_vec();
    let mut executor = FakeExecutor::new(vec![timeout]);

    let report = Verifier::new(&task, TaskStatus::Verify, repository.path(), &mut executor)
        .unwrap()
        .verify()
        .unwrap();

    assert_eq!(report.verdict, VerificationVerdict::Failed);
    assert!(report.results[0].timed_out);
    assert!(!report.results[0].passed);
}

#[test]
fn bounded_output_artifacts_are_preserved() {
    let repository = TestRepository::new();
    let task = contract(&["cargo test"]);
    let mut executor = FakeExecutor::new(vec![execution(Some(0), false, "bounded")]);

    let report = Verifier::new(&task, TaskStatus::Verify, repository.path(), &mut executor)
        .unwrap()
        .verify()
        .unwrap();

    assert_eq!(report.results[0].stdout_artifact_id, "stdout-bounded");
    assert_eq!(report.results[0].stderr_artifact_id, "stderr-bounded");
}

#[test]
fn detects_source_mutation_even_when_file_was_already_dirty() {
    let repository = TestRepository::new();
    let source = repository.path().join("source.txt");
    fs::write(&source, "dirty before\n").unwrap();
    let task = contract(&["cargo test"]);
    let mut planned = execution(Some(0), false, "mutation");
    planned.mutation = Some((source, b"dirty after\n".to_vec()));
    let mut executor = FakeExecutor::new(vec![planned]);

    let error = Verifier::new(&task, TaskStatus::Verify, repository.path(), &mut executor)
        .unwrap()
        .verify()
        .unwrap_err();

    assert_eq!(error, VerifierError::SourceMutation);
}

#[test]
fn detects_untracked_content_mutation() {
    let repository = TestRepository::new();
    let untracked = repository.path().join("untracked.txt");
    fs::write(&untracked, "before\n").unwrap();
    let task = contract(&["cargo test"]);
    let mut planned = execution(Some(0), false, "mutation");
    planned.mutation = Some((untracked, b"after\n".to_vec()));
    let mut executor = FakeExecutor::new(vec![planned]);

    assert_eq!(
        Verifier::new(&task, TaskStatus::Verify, repository.path(), &mut executor)
            .unwrap()
            .verify()
            .unwrap_err(),
        VerifierError::SourceMutation
    );
}

#[test]
fn denied_command_and_artifact_failure_are_typed() {
    for (process_error, verifier_error) in [
        (ProcessError::CommandDenied, VerifierError::CommandDenied),
        (ProcessError::ArtifactFailed, VerifierError::ArtifactFailed),
    ] {
        let repository = TestRepository::new();
        let task = contract(&["cargo test"]);
        let mut executor = FakeExecutor::new(vec![PlannedExecution {
            result: Err(process_error),
            mutation: None,
        }]);
        assert_eq!(
            Verifier::new(&task, TaskStatus::Verify, repository.path(), &mut executor)
                .unwrap()
                .verify()
                .unwrap_err(),
            verifier_error
        );
    }
}

#[test]
fn empty_list_and_shell_syntax_are_rejected() {
    let repository = TestRepository::new();
    let empty = contract(&[]);
    let mut executor = FakeExecutor::new(Vec::new());
    assert!(matches!(
        Verifier::new(&empty, TaskStatus::Verify, repository.path(), &mut executor),
        Err(VerifierError::EmptyVerificationList)
    ));

    let unsafe_command = contract(&["cargo test; echo stolen"]);
    let mut executor = FakeExecutor::new(Vec::new());
    assert_eq!(
        Verifier::new(
            &unsafe_command,
            TaskStatus::Verify,
            repository.path(),
            &mut executor
        )
        .unwrap()
        .verify()
        .unwrap_err(),
        VerifierError::InvalidCommand
    );
}

#[test]
fn verifier_cannot_propose_done() {
    let repository = TestRepository::new();
    let task = contract(&["cargo test"]);
    let mut executor = FakeExecutor::new(vec![execution(Some(0), false, "success")]);

    assert_eq!(
        Verifier::new(
            &task,
            TaskStatus::Integrate,
            repository.path(),
            &mut executor
        )
        .unwrap()
        .verify()
        .unwrap_err(),
        VerifierError::InvalidTransition
    );
}

#[test]
fn errors_do_not_expose_output_or_host_path() {
    let repository = TestRepository::new();
    let task = contract(&["cargo test"]);
    let secret = "secret-output-marker";
    let mut executor = FakeExecutor::new(vec![PlannedExecution {
        result: Err(ProcessError::ContainerFailed),
        mutation: None,
    }]);

    let error = Verifier::new(&task, TaskStatus::Verify, repository.path(), &mut executor)
        .unwrap()
        .verify()
        .unwrap_err();
    let rendered = format!("{error} {error:?}");

    assert!(!rendered.contains(secret));
    assert!(!rendered.contains(repository.path().to_str().unwrap()));
}

#[test]
fn artifact_targets_are_deterministic_and_contain_no_command_text() {
    let repository = TestRepository::new();
    let task = contract(&["cargo test"]);
    let mut first = FakeExecutor::new(vec![execution(Some(0), false, "first")]);
    Verifier::new(&task, TaskStatus::Verify, repository.path(), &mut first)
        .unwrap()
        .verify()
        .unwrap();
    let mut second = FakeExecutor::new(vec![execution(Some(0), false, "second")]);
    Verifier::new(&task, TaskStatus::Verify, repository.path(), &mut second)
        .unwrap()
        .verify()
        .unwrap();

    assert_eq!(first.targets, second.targets);
    assert!(!first.targets[0].stdout_id.contains("cargo"));
}
