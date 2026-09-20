use std::{
    cell::Cell,
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ai_team::{
    agent::worker::{
        CheckpointStore, StopReason, ToolCheckpoint, Worker, WorkerClock, WorkerConfig,
        WorkerError, WorkerModel, WorkerRun,
    },
    context::{ContextBuilder, ContextLimits},
    domain::task::{
        AllowedPath, MaxAttempts, NonEmptyString, PositiveLimit, TaskContract, TaskLimits,
        TaskStatus,
    },
    model::{FinishReason, ModelRequest, ModelResponse, ToolCall, Usage},
    runner::{
        git::{GitWorktreeManager, Worktree},
        policy::{ToolPolicy, ToolRole},
        tools::StructuredTools,
    },
    store::artifact::ArtifactStore,
};
use serde_json::json;

#[derive(Clone, Default)]
struct TestClock(Rc<Cell<Duration>>);

impl TestClock {
    fn advance(&self, duration: Duration) {
        self.0.set(self.0.get() + duration);
    }
}

impl WorkerClock for TestClock {
    fn elapsed(&self) -> Duration {
        self.0.get()
    }
}

struct ScriptedModel {
    responses: VecDeque<Result<ModelResponse, ()>>,
    advances: VecDeque<Duration>,
    clock: Option<TestClock>,
    calls: usize,
}

impl WorkerModel for ScriptedModel {
    type Error = ();

    fn complete(&mut self, _: &ModelRequest) -> Result<ModelResponse, Self::Error> {
        self.calls += 1;
        if let (Some(clock), Some(duration)) = (&self.clock, self.advances.pop_front()) {
            clock.advance(duration);
        }
        self.responses.pop_front().unwrap_or(Err(()))
    }
}

#[derive(Default)]
struct MemoryCheckpoints {
    values: HashMap<String, ToolCheckpoint>,
    fail_save: bool,
    advance_on_save: Option<(TestClock, Duration)>,
    saves: usize,
}

impl CheckpointStore for MemoryCheckpoints {
    type Error = ();

    fn completed(&self, call_id: &str) -> Option<ToolCheckpoint> {
        self.values.get(call_id).cloned()
    }

    fn save(&mut self, checkpoint: &ToolCheckpoint) -> Result<(), Self::Error> {
        self.saves += 1;
        if self.fail_save {
            return Err(());
        }
        self.values
            .insert(checkpoint.call_id.clone(), checkpoint.clone());
        if let Some((clock, duration)) = &self.advance_on_save {
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

fn model(responses: Vec<Result<ModelResponse, ()>>) -> ScriptedModel {
    ScriptedModel {
        responses: responses.into(),
        advances: VecDeque::new(),
        clock: None,
        calls: 0,
    }
}

fn timed_model(
    responses: Vec<Result<ModelResponse, ()>>,
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

fn run(
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
}

fn run_with_clock(
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
}

#[test]
fn edit_multiple_calls_checkpoint_and_complete_handoff() {
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
    );

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

#[test]
fn duplicate_call_id_is_not_executed_again() {
    let fixture = Fixture::new();
    let request = call("same", "request_human", json!({"message":"first"}));
    let mut checkpoints = MemoryCheckpoints::default();
    checkpoints.values.insert(
        "same".into(),
        ToolCheckpoint {
            call_id: "same".into(),
            tool: "request_human".into(),
            succeeded: true,
            error_code: None,
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
    );
    assert_eq!(result.handoff.stop_reason, StopReason::Completed);
    assert_eq!(result.handoff.token_usage.tool_calls, 0);
    assert_eq!(result.checkpoints.saves, 0);
}

#[test]
fn budgets_and_turn_limit_stop_loop() {
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
        );
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
    );
    assert_eq!(turns.handoff.stop_reason, StopReason::MaxTurns);
}

#[test]
fn timeout_model_tool_and_human_stops_are_typed() {
    let fixture = Fixture::new();
    let task = contract(100, 100, 10);
    let timeout = run(
        &fixture,
        &task,
        model(Vec::new()),
        MemoryCheckpoints::default(),
        2,
        Duration::ZERO,
    );
    assert_eq!(timeout.handoff.stop_reason, StopReason::Timeout);

    let model_error = run(
        &fixture,
        &task,
        model(vec![Err(()), Err(())]),
        MemoryCheckpoints::default(),
        2,
        Duration::from_secs(5),
    );
    assert_eq!(model_error.handoff.stop_reason, StopReason::ModelError);
    assert_eq!(model_error.error, Some(WorkerError::Model));

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
    );
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
    );
    assert_eq!(human.handoff.stop_reason, StopReason::HumanRequested);
    assert_eq!(human.handoff.summary, "review");
}

#[test]
fn checkpoint_failure_after_side_effect_is_fatal() {
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
            fail_save: true,
            ..MemoryCheckpoints::default()
        },
        3,
        Duration::from_secs(5),
    );
    assert_eq!(result.handoff.stop_reason, StopReason::ToolError);
    assert_eq!(result.error, Some(WorkerError::Checkpoint));
    assert_eq!(result.model.calls, 1);
    assert_eq!(
        fs::read_to_string(fixture.worktree.path().join("src/file.txt")).unwrap(),
        "changed-once\n"
    );
}

#[test]
fn late_model_response_is_ignored_even_when_it_claims_completion() {
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
    );

    assert_eq!(result.handoff.stop_reason, StopReason::Timeout);
    assert_eq!(result.handoff.next_status, None);
    assert!(result.handoff.summary.is_empty());
    assert_eq!(result.handoff.token_usage.input_tokens, 0);
    let rendered = format!("{:?}", result.error);
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains(fixture.root.to_string_lossy().as_ref()));
}

#[test]
fn late_model_tool_call_is_never_executed() {
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
    );

    assert_eq!(result.handoff.stop_reason, StopReason::Timeout);
    assert_eq!(result.handoff.token_usage.tool_calls, 0);
    assert_eq!(result.checkpoints.saves, 0);
    assert_eq!(
        fs::read_to_string(fixture.worktree.path().join("src/file.txt")).unwrap(),
        "old\n"
    );
}

#[test]
fn late_tool_is_checkpointed_then_stops_before_second_tool() {
    let fixture = Fixture::new();
    let clock = TestClock::default();
    let patch = "diff --git a/src/file.txt b/src/file.txt\n--- a/src/file.txt\n+++ b/src/file.txt\n@@ -1 +1 @@\n-old\n+late-change\n";
    let checkpoints = MemoryCheckpoints {
        advance_on_save: Some((clock.clone(), Duration::from_secs(2))),
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
    );

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

#[test]
fn late_human_request_becomes_timeout_after_checkpoint() {
    let fixture = Fixture::new();
    let clock = TestClock::default();
    let checkpoints = MemoryCheckpoints {
        advance_on_save: Some((clock.clone(), Duration::from_secs(2))),
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
    );

    assert_eq!(result.handoff.stop_reason, StopReason::Timeout);
    assert!(result.handoff.summary.is_empty());
    assert_eq!(result.checkpoints.saves, 1);
    assert!(result.checkpoints.values.contains_key("late-human"));
}

#[test]
fn done_and_sensitive_model_content_are_rejected_without_leak() {
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
    );
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
