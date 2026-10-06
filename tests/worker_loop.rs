use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ai_team::{
    agent::worker::{
        CheckpointOutcome, CheckpointReservation, CheckpointStore, StopReason, ToolCheckpoint,
        Worker, WorkerClock, WorkerConfig, WorkerError, WorkerModel, WorkerRun,
    },
    context::{ContextBuilder, ContextLimits},
    domain::task::{
        AllowedPath, MaxAttempts, NonEmptyString, PositiveLimit, TaskContract, TaskLimits,
        TaskStatus,
    },
    model::{
        FinishReason, ModelError, ModelErrorKind, ModelRequest, ModelResponse, ToolCall, Usage,
    },
    runner::{
        git::{GitWorktreeManager, Worktree},
        policy::{ToolPolicy, ToolRole},
        tools::StructuredTools,
    },
    store::artifact::ArtifactStore,
};
use serde_json::json;

#[derive(Clone, Default)]
struct TestClock(Arc<Mutex<Duration>>);

impl TestClock {
    fn advance(&self, duration: Duration) {
        *self.0.lock().unwrap() += duration;
    }
}

impl WorkerClock for TestClock {
    fn elapsed(&self) -> Duration {
        *self.0.lock().unwrap()
    }
}

struct ScriptedModel {
    responses: VecDeque<Result<ModelResponse, ModelError>>,
    advances: VecDeque<Duration>,
    clock: Option<TestClock>,
    calls: usize,
}

impl WorkerModel for ScriptedModel {
    async fn complete(&mut self, _: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.calls += 1;
        if let (Some(clock), Some(duration)) = (&self.clock, self.advances.pop_front()) {
            clock.advance(duration);
        }
        self.responses
            .pop_front()
            .unwrap_or_else(|| Err(ModelError::new(ModelErrorKind::ProviderUnavailable)))
    }
}

#[derive(Default)]
struct MemoryCheckpoints {
    values: HashMap<String, ToolCheckpoint>,
    in_progress: HashMap<String, String>,
    fail_reserve: bool,
    fail_complete: bool,
    advance_on_complete: Option<(TestClock, Duration)>,
    reservations: usize,
    saves: usize,
    observe_path: Option<PathBuf>,
    side_effect_seen_at_reserve: bool,
}

impl CheckpointStore for MemoryCheckpoints {
    type Error = ();

    async fn reserve(
        &mut self,
        _: &str,
        call_id: &str,
        tool: &str,
    ) -> Result<CheckpointReservation, Self::Error> {
        self.reservations += 1;
        if let Some(path) = &self.observe_path {
            self.side_effect_seen_at_reserve =
                fs::read_to_string(path).is_ok_and(|content| content != "old\n");
        }
        if self.fail_reserve {
            return Err(());
        }
        if let Some(checkpoint) = self.values.get(call_id) {
            return Ok(CheckpointReservation::Completed(checkpoint.clone()));
        }
        if self.in_progress.contains_key(call_id) {
            return Ok(CheckpointReservation::InProgress);
        }
        self.in_progress.insert(call_id.into(), tool.into());
        Ok(CheckpointReservation::New)
    }

    async fn complete(&mut self, _: &str, checkpoint: &ToolCheckpoint) -> Result<(), Self::Error> {
        self.saves += 1;
        if self.fail_complete {
            return Err(());
        }
        self.in_progress.remove(&checkpoint.call_id);
        self.values
            .insert(checkpoint.call_id.clone(), checkpoint.clone());
        if let Some((clock, duration)) = &self.advance_on_complete {
            clock.advance(*duration);
        }
        Ok(())
    }
}

struct Fixture {
    root: PathBuf,
    manager: GitWorktreeManager,
    worktree: Worktree,
    artifacts: ArtifactStore,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "noctis-worker-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repository = root.join("repository");
        fs::create_dir_all(repository.join("src")).unwrap();
        fs::write(repository.join("src/file.txt"), "old\n").unwrap();
        git(&repository, &["init", "--initial-branch=main"]);
        git(&repository, &["config", "user.name", "Noctis Test"]);
        git(
            &repository,
            &["config", "user.email", "noctis@example.invalid"],
        );
        git(&repository, &["add", "."]);
        git(&repository, &["commit", "-m", "initial"]);
        let base = git_output(&repository, &["rev-parse", "HEAD"]);
        let manager = GitWorktreeManager::new(&repository, root.join("worktrees")).unwrap();
        let worktree = manager.create("worker", "worker-test", &base).unwrap();
        let artifacts = ArtifactStore::new(root.join("artifacts"), 64 * 1024).unwrap();
        Self {
            root,
            manager,
            worktree,
            artifacts,
        }
    }

    fn tools(&self) -> StructuredTools<'_> {
        let policy = ToolPolicy::new(
            ToolRole::Worker,
            vec!["src/**".into()],
            64 * 1024,
            64 * 1024,
            Duration::from_secs(2),
        )
        .unwrap();
        StructuredTools::new(policy, &self.manager, &self.worktree, &self.artifacts)
    }

    fn context(&self) -> ContextBuilder<'_> {
        ContextBuilder::new(
            self.worktree.path(),
            &self.artifacts,
            ContextLimits {
                max_bytes: 16_000,
                max_tokens: 4_000,
                max_file_bytes: 4_000,
                max_command_output_bytes: 4_000,
            },
        )
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.manager.cleanup(&self.worktree);
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn contract(input: i64, output: i64, calls: i64) -> TaskContract {
    TaskContract {
        id: text("id", "M2-009"),
        project_id: text("project_id", "noctis"),
        project_run_id: text("project_run_id", "run-1"),
        title: text("title", "Worker loop"),
        role: text("role", "worker"),
        objective: text("objective", "Run model turns safely"),
        depends_on: Vec::new(),
        allowed_paths: vec![AllowedPath::parse("src/**").unwrap()],
        context_refs: Vec::new(),
        acceptance_criteria: vec![text("acceptance_criteria", "tests pass")],
        verification_commands: vec![text("verification_commands", "cargo test")],
        limits: TaskLimits {
            max_input_tokens: PositiveLimit::new("max_input_tokens", input).unwrap(),
            max_output_tokens: PositiveLimit::new("max_output_tokens", output).unwrap(),
            max_tool_calls: PositiveLimit::new("max_tool_calls", calls).unwrap(),
            max_attempts: MaxAttempts::new(2).unwrap(),
            timeout_seconds: PositiveLimit::new("timeout_seconds", 60).unwrap(),
        },
    }
}

fn text(field: &'static str, value: &str) -> NonEmptyString {
    NonEmptyString::parse(field, value).unwrap()
}

fn response(content: Option<&str>, calls: Vec<ToolCall>, input: u64, output: u64) -> ModelResponse {
    ModelResponse {
        content: content.map(str::to_owned),
        tool_calls: calls,
        finish_reason: if content.is_some() {
            FinishReason::Stop
        } else {
            FinishReason::ToolCalls
        },
        usage: Usage {
            input_tokens: input,
            output_tokens: output,
            cached_tokens: 0,
            total_tokens: input + output,
            estimated: false,
        },
        latency_ms: 1,
    }
}

fn call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
    }
}

fn model(responses: Vec<Result<ModelResponse, ModelError>>) -> ScriptedModel {
    ScriptedModel {
        responses: responses.into(),
        advances: VecDeque::new(),
        clock: None,
        calls: 0,
    }
}

fn timed_model(
    responses: Vec<Result<ModelResponse, ModelError>>,
    clock: TestClock,
    advances: Vec<Duration>,
) -> ScriptedModel {
    ScriptedModel {
        responses: responses.into(),
        advances: advances.into(),
        clock: Some(clock),
        calls: 0,
    }
}

fn config(max_turns: u32, deadline: Duration) -> WorkerConfig {
    WorkerConfig {
        agent_run_id: "agent-run-1".into(),
        model_class: "coding".into(),
        max_turns,
        deadline,
    }
}

async fn run(
    fixture: &Fixture,
    task: &TaskContract,
    scripted: ScriptedModel,
    checkpoints: MemoryCheckpoints,
    max_turns: u32,
    deadline: Duration,
) -> WorkerRun<ScriptedModel, MemoryCheckpoints> {
    let context = fixture.context();
    let tools = fixture.tools();
    Worker::new(
        task,
        TaskStatus::Running,
        &context,
        &tools,
        scripted,
        checkpoints,
        config(max_turns, deadline),
    )
    .run()
    .await
}

async fn run_with_clock(
    fixture: &Fixture,
    task: &TaskContract,
    scripted: ScriptedModel,
    checkpoints: MemoryCheckpoints,
    max_turns: u32,
    deadline: Duration,
    clock: TestClock,
) -> WorkerRun<ScriptedModel, MemoryCheckpoints> {
    let context = fixture.context();
    let tools = fixture.tools();
    Worker::with_clock(
        task,
        TaskStatus::Running,
        &context,
        &tools,
        scripted,
        checkpoints,
        config(max_turns, deadline),
        clock,
    )
    .run()
    .await
}

#[tokio::test]
async fn edit_multiple_calls_checkpoint_and_complete_handoff() {
    let fixture = Fixture::new();
    let patch = "diff --git a/src/file.txt b/src/file.txt\n--- a/src/file.txt\n+++ b/src/file.txt\n@@ -1 +1 @@\n-old\n+new\n";
    let first = response(
        None,
        vec![
            call("patch-1", "apply_patch", json!({"patch": patch})),
            call("status-1", "git_status", json!({})),
        ],
        10,
        5,
    );
    let done = response(
        Some(r#"{"summary":"edited","status":"self_check"}"#),
        Vec::new(),
        10,
        5,
    );
    let result = run(
        &fixture,
        &contract(100, 100, 10),
        model(vec![Ok(first), Ok(done)]),
        MemoryCheckpoints::default(),
        4,
        Duration::from_secs(5),
    )
    .await;

    assert_eq!(result.handoff.stop_reason, StopReason::Completed);
    assert_eq!(result.handoff.next_status, Some(TaskStatus::SelfCheck));
    assert_eq!(result.handoff.summary, "edited");
    assert_eq!(result.handoff.changed_paths, ["src/file.txt"]);
    assert_eq!(result.handoff.verification_commands, ["cargo test"]);
    assert_eq!(result.handoff.token_usage.tool_calls, 2);
    assert_eq!(result.checkpoints.saves, 2);
    assert_eq!(
        fs::read_to_string(fixture.worktree.path().join("src/file.txt")).unwrap(),
        "new\n"
    );
}

#[tokio::test]
async fn duplicate_call_id_is_not_executed_again() {
    let fixture = Fixture::new();
    let request = call("same", "request_human", json!({"message":"first"}));
    let mut checkpoints = MemoryCheckpoints::default();
    checkpoints.values.insert(
        "same".into(),
        ToolCheckpoint {
            call_id: "same".into(),
            outcome: CheckpointOutcome::Succeeded,
            duration_ms: 0,
            artifact_id: None,
        },
    );
    let result = run(
        &fixture,
        &contract(100, 100, 10),
        model(vec![
            Ok(response(None, vec![request], 1, 1)),
            Ok(response(
                Some(r#"{"summary":"done","status":"self_check"}"#),
                Vec::new(),
                1,
                1,
            )),
        ]),
        checkpoints,
        3,
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(result.handoff.stop_reason, StopReason::Completed);
    assert_eq!(result.handoff.token_usage.tool_calls, 0);
    assert_eq!(result.checkpoints.saves, 0);
}

#[tokio::test]
async fn budgets_and_turn_limit_stop_loop() {
    let fixture = Fixture::new();
    for (task, scripted) in [
        (
            contract(5, 10, 10),
            model(vec![Ok(response(None, Vec::new(), 6, 1))]),
        ),
        (
            contract(10, 5, 10),
            model(vec![Ok(response(None, Vec::new(), 1, 6))]),
        ),
        (
            contract(10, 10, 1),
            model(vec![Ok(response(
                None,
                vec![
                    call("one", "git_status", json!({})),
                    call("two", "git_status", json!({})),
                ],
                1,
                1,
            ))]),
        ),
    ] {
        let result = run(
            &fixture,
            &task,
            scripted,
            MemoryCheckpoints::default(),
            2,
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(result.handoff.stop_reason, StopReason::BudgetExhausted);
    }
    let turns = run(
        &fixture,
        &contract(10, 10, 10),
        model(vec![Ok(response(
            None,
            vec![call("status", "git_status", json!({}))],
            1,
            1,
        ))]),
        MemoryCheckpoints::default(),
        1,
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(turns.handoff.stop_reason, StopReason::MaxTurns);
}

#[tokio::test]
async fn timeout_model_tool_and_human_stops_are_typed() {
    let fixture = Fixture::new();
    let task = contract(100, 100, 10);
    let timeout = run(
        &fixture,
        &task,
        model(Vec::new()),
        MemoryCheckpoints::default(),
        2,
        Duration::ZERO,
    )
    .await;
    assert_eq!(timeout.handoff.stop_reason, StopReason::Timeout);

    let model_error = run(
        &fixture,
        &task,
        model(vec![
            Err(ModelError::new(ModelErrorKind::RateLimited)),
            Err(ModelError::new(ModelErrorKind::RateLimited)),
        ]),
        MemoryCheckpoints::default(),
        2,
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(model_error.handoff.stop_reason, StopReason::ModelError);
    assert_eq!(
        model_error.error,
        Some(WorkerError::Model(ModelErrorKind::RateLimited))
    );

    let tool_error = run(
        &fixture,
        &task,
        model(vec![Ok(response(
            None,
            vec![call("bad", "read_file", json!({"path":"denied.txt"}))],
            1,
            1,
        ))]),
        MemoryCheckpoints::default(),
        2,
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(tool_error.handoff.stop_reason, StopReason::ToolError);
    assert!(matches!(tool_error.error, Some(WorkerError::Tool(_))));

    let human = run(
        &fixture,
        &task,
        model(vec![Ok(response(
            None,
            vec![call("human", "request_human", json!({"message":"review"}))],
            1,
            1,
        ))]),
        MemoryCheckpoints::default(),
        2,
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(human.handoff.stop_reason, StopReason::HumanRequested);
    assert_eq!(human.handoff.summary, "review");
}

#[tokio::test]
async fn checkpoint_failure_after_side_effect_is_fatal() {
    let fixture = Fixture::new();
    let patch = "diff --git a/src/file.txt b/src/file.txt\n--- a/src/file.txt\n+++ b/src/file.txt\n@@ -1 +1 @@\n-old\n+changed-once\n";
    let result = run(
        &fixture,
        &contract(100, 100, 10),
        model(vec![Ok(response(
            None,
            vec![call("patch", "apply_patch", json!({"patch":patch}))],
            1,
            1,
        ))]),
        MemoryCheckpoints {
            fail_complete: true,
            ..MemoryCheckpoints::default()
        },
        3,
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(result.handoff.stop_reason, StopReason::ToolError);
    assert_eq!(result.error, Some(WorkerError::CheckpointCompletion));
    assert_eq!(result.model.calls, 1);
    assert_eq!(
        fs::read_to_string(fixture.worktree.path().join("src/file.txt")).unwrap(),
        "changed-once\n"
    );
}

#[tokio::test]
async fn late_model_response_is_ignored_even_when_it_claims_completion() {
    let fixture = Fixture::new();
    let clock = TestClock::default();
    let secret = "LATE_SECRET /absolute/host/path patch-content";
    let scripted = timed_model(
        vec![Ok(response(
            Some(&format!(
                r#"{{"summary":"{secret}","status":"self_check"}}"#
            )),
            Vec::new(),
            1,
            1,
        ))],
        clock.clone(),
        vec![Duration::from_secs(2)],
    );
    let result = run_with_clock(
        &fixture,
        &contract(100, 100, 10),
        scripted,
        MemoryCheckpoints::default(),
        2,
        Duration::from_secs(1),
        clock,
    )
    .await;

    assert_eq!(result.handoff.stop_reason, StopReason::Timeout);
    assert_eq!(result.handoff.next_status, None);
    assert!(result.handoff.summary.is_empty());
    assert_eq!(result.handoff.token_usage.input_tokens, 0);
    let rendered = format!("{:?}", result.error);
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains(fixture.root.to_string_lossy().as_ref()));
}

#[tokio::test]
async fn late_model_tool_call_is_never_executed() {
    let fixture = Fixture::new();
    let clock = TestClock::default();
    let patch = "diff --git a/src/file.txt b/src/file.txt\n--- a/src/file.txt\n+++ b/src/file.txt\n@@ -1 +1 @@\n-old\n+must-not-change\n";
    let scripted = timed_model(
        vec![Ok(response(
            None,
            vec![call("late-tool", "apply_patch", json!({"patch":patch}))],
            1,
            1,
        ))],
        clock.clone(),
        vec![Duration::from_secs(2)],
    );
    let result = run_with_clock(
        &fixture,
        &contract(100, 100, 10),
        scripted,
        MemoryCheckpoints::default(),
        2,
        Duration::from_secs(1),
        clock,
    )
    .await;

    assert_eq!(result.handoff.stop_reason, StopReason::Timeout);
    assert_eq!(result.handoff.token_usage.tool_calls, 0);
    assert_eq!(result.checkpoints.saves, 0);
    assert_eq!(
        fs::read_to_string(fixture.worktree.path().join("src/file.txt")).unwrap(),
        "old\n"
    );
}

#[tokio::test]
async fn late_tool_is_checkpointed_then_stops_before_second_tool() {
    let fixture = Fixture::new();
    let clock = TestClock::default();
    let patch = "diff --git a/src/file.txt b/src/file.txt\n--- a/src/file.txt\n+++ b/src/file.txt\n@@ -1 +1 @@\n-old\n+late-change\n";
    let checkpoints = MemoryCheckpoints {
        advance_on_complete: Some((clock.clone(), Duration::from_secs(2))),
        ..MemoryCheckpoints::default()
    };
    let result = run_with_clock(
        &fixture,
        &contract(100, 100, 10),
        model(vec![Ok(response(
            None,
            vec![
                call("late-patch", "apply_patch", json!({"patch":patch})),
                call("must-not-run", "request_human", json!({"message":"secret"})),
            ],
            1,
            1,
        ))]),
        checkpoints,
        2,
        Duration::from_secs(1),
        clock,
    )
    .await;

    assert_eq!(result.handoff.stop_reason, StopReason::Timeout);
    assert_eq!(result.handoff.token_usage.tool_calls, 1);
    assert_eq!(result.checkpoints.saves, 1);
    assert!(result.checkpoints.values.contains_key("late-patch"));
    assert!(!result.checkpoints.values.contains_key("must-not-run"));
    assert_eq!(
        fs::read_to_string(fixture.worktree.path().join("src/file.txt")).unwrap(),
        "late-change\n"
    );
}

#[tokio::test]
async fn late_human_request_becomes_timeout_after_checkpoint() {
    let fixture = Fixture::new();
    let clock = TestClock::default();
    let checkpoints = MemoryCheckpoints {
        advance_on_complete: Some((clock.clone(), Duration::from_secs(2))),
        ..MemoryCheckpoints::default()
    };
    let result = run_with_clock(
        &fixture,
        &contract(100, 100, 10),
        model(vec![Ok(response(
            None,
            vec![call(
                "late-human",
                "request_human",
                json!({"message":"do not expose"}),
            )],
            1,
            1,
        ))]),
        checkpoints,
        2,
        Duration::from_secs(1),
        clock,
    )
    .await;

    assert_eq!(result.handoff.stop_reason, StopReason::Timeout);
    assert!(result.handoff.summary.is_empty());
    assert_eq!(result.checkpoints.saves, 1);
    assert!(result.checkpoints.values.contains_key("late-human"));
}

#[tokio::test]
async fn done_and_sensitive_model_content_are_rejected_without_leak() {
    let fixture = Fixture::new();
    let secret = "SECRET_MARKER patch /absolute/host/path";
    let result = run(
        &fixture,
        &contract(100, 100, 10),
        model(vec![Ok(response(
            Some(&format!(r#"{{"summary":"{secret}","status":"done"}}"#)),
            Vec::new(),
            1,
            1,
        ))]),
        MemoryCheckpoints::default(),
        2,
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(result.handoff.stop_reason, StopReason::ModelError);
    assert_eq!(result.error, Some(WorkerError::InvalidModelResponse));
    assert!(result.handoff.summary.is_empty());
    let error = result.error.as_ref().unwrap();
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains(fixture.root.to_string_lossy().as_ref()));
}

fn git(repository: &Path, arguments: &[&str]) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(arguments)
            .status()
            .unwrap()
            .success()
    );
}

fn git_output(repository: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().into()
}

#[tokio::test]
async fn in_progress_reservation_stops_without_repeating_side_effect() {
    let fixture = Fixture::new();
    let patch = "diff --git a/src/file.txt b/src/file.txt\n--- a/src/file.txt\n+++ b/src/file.txt\n@@ -1 +1 @@\n-old\n+repeated\n";
    let mut checkpoints = MemoryCheckpoints::default();
    checkpoints
        .in_progress
        .insert("patch".into(), "apply_patch".into());

    let result = run(
        &fixture,
        &contract(100, 100, 10),
        model(vec![Ok(response(
            None,
            vec![call("patch", "apply_patch", json!({"patch":patch}))],
            1,
            1,
        ))]),
        checkpoints,
        2,
        Duration::from_secs(5),
    )
    .await;

    assert_eq!(result.handoff.stop_reason, StopReason::RecoveryRequired);
    assert_eq!(result.error, Some(WorkerError::AmbiguousToolCall));
    assert_eq!(result.checkpoints.saves, 0);
    assert_eq!(
        fs::read_to_string(fixture.worktree.path().join("src/file.txt")).unwrap(),
        "old\n"
    );
}

#[tokio::test]
async fn reservation_is_persisted_before_side_effect() {
    let fixture = Fixture::new();
    let path = fixture.worktree.path().join("src/file.txt");
    let patch = "diff --git a/src/file.txt b/src/file.txt\n--- a/src/file.txt\n+++ b/src/file.txt\n@@ -1 +1 @@\n-old\n+changed\n";
    let result = run(
        &fixture,
        &contract(100, 100, 10),
        model(vec![Ok(response(
            None,
            vec![call("patch", "apply_patch", json!({"patch":patch}))],
            1,
            1,
        ))]),
        MemoryCheckpoints {
            observe_path: Some(path),
            ..MemoryCheckpoints::default()
        },
        1,
        Duration::from_secs(5),
    )
    .await;

    assert_eq!(result.checkpoints.reservations, 1);
    assert!(!result.checkpoints.side_effect_seen_at_reserve);
    assert_eq!(result.checkpoints.saves, 1);
}

#[tokio::test]
async fn reservation_failure_stops_before_side_effect() {
    let fixture = Fixture::new();
    let patch = "diff --git a/src/file.txt b/src/file.txt\n--- a/src/file.txt\n+++ b/src/file.txt\n@@ -1 +1 @@\n-old\n+changed\n";
    let result = run(
        &fixture,
        &contract(100, 100, 10),
        model(vec![Ok(response(
            None,
            vec![call("patch", "apply_patch", json!({"patch":patch}))],
            1,
            1,
        ))]),
        MemoryCheckpoints {
            fail_reserve: true,
            ..MemoryCheckpoints::default()
        },
        1,
        Duration::from_secs(5),
    )
    .await;

    assert_eq!(result.error, Some(WorkerError::CheckpointReservation));
    assert_eq!(result.checkpoints.saves, 0);
    assert_eq!(
        fs::read_to_string(fixture.worktree.path().join("src/file.txt")).unwrap(),
        "old\n"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn production_tool_identity_mismatch_stops_before_execution(pool: sqlx::PgPool) {
    use ai_team::{
        domain::state_machine::Actor,
        store::{event::ClaimAttempt, task::TaskRepository},
    };
    use uuid::Uuid;

    let project_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO projects (id,name,repository_path) VALUES ($1,'worker','/repository')",
    )
    .bind(project_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'worker','running',100)")
        .bind(run_id).bind(project_id).execute(&pool).await.unwrap();
    let mut task = contract(100, 100, 10);
    task.project_id = text("project_id", &project_id.to_string());
    task.project_run_id = text("project_run_id", &run_id.to_string());
    let mut repository = TaskRepository::new(pool);
    repository.create(&task).await.unwrap();
    repository
        .transition(task.id.as_str(), 0, TaskStatus::Planned, Actor::System)
        .await
        .unwrap();
    repository
        .transition(task.id.as_str(), 1, TaskStatus::Ready, Actor::System)
        .await
        .unwrap();
    let attempt_id = Uuid::new_v4();
    repository
        .claim_ready(
            2,
            &ClaimAttempt {
                id: attempt_id,
                task_id: task.id.clone(),
                role: task.role.clone(),
                provider_id: text("provider_id", "provider"),
                model_id: text("model_id", "model"),
                branch: "worker-test".into(),
                base_commit: "0123456789abcdef0123456789abcdef01234567".into(),
                retention_seconds: 3600,
            },
        )
        .await
        .unwrap();
    let agent_run_id = attempt_id.to_string();
    repository
        .reserve(&agent_run_id, "patch", "git_status")
        .await
        .unwrap();
    let fixture = Fixture::new();
    let context = fixture.context();
    let tools = fixture.tools();
    let patch = "diff --git a/src/file.txt b/src/file.txt\n--- a/src/file.txt\n+++ b/src/file.txt\n@@ -1 +1 @@\n-old\n+changed\n";
    for completed in [false, true] {
        if completed {
            repository
                .complete(
                    &agent_run_id,
                    &ToolCheckpoint {
                        call_id: "patch".into(),
                        outcome: CheckpointOutcome::Succeeded,
                        duration_ms: 1,
                        artifact_id: None,
                    },
                )
                .await
                .unwrap();
        }
        let mut configuration = config(1, Duration::from_secs(5));
        configuration.agent_run_id = agent_run_id.clone();
        let result = Worker::new(
            &task,
            TaskStatus::Running,
            &context,
            &tools,
            model(vec![Ok(response(
                None,
                vec![call("patch", "apply_patch", json!({"patch":patch}))],
                1,
                1,
            ))]),
            repository.clone(),
            configuration,
        )
        .run()
        .await;
        assert_eq!(result.error, Some(WorkerError::CheckpointReservation));
        assert_eq!(
            fs::read_to_string(fixture.worktree.path().join("src/file.txt")).unwrap(),
            "old\n"
        );
    }
}

#[tokio::test]
async fn usage_is_aggregated_losslessly() {
    let fixture = Fixture::new();
    let mut first = response(None, vec![call("status", "git_status", json!({}))], 10, 5);
    first.usage.cached_tokens = 3;
    first.usage.estimated = true;
    first.latency_ms = 7;
    let mut done = response(
        Some(r#"{"summary":"done","status":"self_check"}"#),
        Vec::new(),
        20,
        6,
    );
    done.usage.cached_tokens = 4;
    done.latency_ms = 11;

    let result = run(
        &fixture,
        &contract(100, 100, 10),
        model(vec![Ok(first), Ok(done)]),
        MemoryCheckpoints::default(),
        2,
        Duration::from_secs(5),
    )
    .await;

    assert_eq!(result.handoff.token_usage.input_tokens, 30);
    assert_eq!(result.handoff.token_usage.cached_tokens, 7);
    assert_eq!(result.handoff.token_usage.output_tokens, 11);
    assert_eq!(result.handoff.token_usage.tool_calls, 1);
    assert_eq!(result.handoff.token_usage.latency_ms, 18);
    assert!(result.handoff.token_usage.estimated);
}

#[tokio::test]
async fn usage_overflow_stops_typed() {
    let fixture = Fixture::new();
    let mut first = response(None, vec![call("status", "git_status", json!({}))], 1, 1);
    first.latency_ms = u128::MAX;
    let second = response(
        Some(r#"{"summary":"done","status":"self_check"}"#),
        Vec::new(),
        1,
        1,
    );

    let result = run(
        &fixture,
        &contract(100, 100, 10),
        model(vec![Ok(first), Ok(second)]),
        MemoryCheckpoints::default(),
        2,
        Duration::from_secs(5),
    )
    .await;

    assert_eq!(result.handoff.stop_reason, StopReason::UsageOverflow);
    assert_eq!(result.error, Some(WorkerError::UsageOverflow));
}
