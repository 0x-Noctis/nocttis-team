mod agent {
    pub mod worker {
        pub use ai_team::agent::worker::*;
    }
}
mod domain {
    pub use ai_team::domain::*;
}
mod model {
    pub use ai_team::model::*;
}
mod runner {
    pub use ai_team::runner::*;
}
mod store {
    pub use ai_team::store::*;
}

#[path = "../src/agent/reviewer.rs"]
mod reviewer;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use ai_team::{
    agent::worker::{StopReason, TokenUsage, WorkerHandoff},
    domain::task::{
        AllowedPath, MaxAttempts, NonEmptyString, PositiveLimit, TaskContract, TaskLimits,
        TaskStatus,
    },
    model::{FinishReason, ModelRequest, ModelResponse, Usage},
    runner::git::{GitWorktreeManager, Worktree},
    store::artifact::ArtifactStore,
};
use reviewer::{
    ReviewDecision, Reviewer, ReviewerError, ReviewerModel, SourceExcerpt, VerificationEvidence,
};

struct ScriptedModel {
    response: Option<Result<ModelResponse, ()>>,
    mutation: Option<Mutation>,
    request: Option<ModelRequest>,
}

enum Mutation {
    Write(PathBuf, String),
    Delete(PathBuf),
}

impl ReviewerModel for ScriptedModel {
    type Error = ();

    fn complete(&mut self, request: &ModelRequest) -> Result<ModelResponse, Self::Error> {
        self.request = Some(request.clone());
        match &self.mutation {
            Some(Mutation::Write(path, content)) => fs::write(path, content).unwrap(),
            Some(Mutation::Delete(path)) => fs::remove_file(path).unwrap(),
            None => {}
        }
        self.response.take().unwrap_or(Err(()))
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
            "noctis-reviewer-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repository = root.join("repository");
        fs::create_dir_all(repository.join("src")).unwrap();
        fs::write(repository.join("src/file.rs"), "fn value() -> u8 { 1 }\n").unwrap();
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
        let worktree = manager.create("review", "review-test", &base).unwrap();
        fs::write(
            worktree.path().join("src/file.rs"),
            "fn value() -> u8 { 2 }\n",
        )
        .unwrap();
        let artifacts = ArtifactStore::new(root.join("artifacts"), 128 * 1024).unwrap();
        artifacts
            .write(
                "verification",
                "cargo-test.txt",
                "text/plain",
                b"all tests passed",
                |_| Ok::<_, ()>(()),
            )
            .unwrap();
        Self {
            root,
            manager,
            worktree,
            artifacts,
        }
    }

    fn review(&self, body: &str) -> reviewer::ReviewRun<ScriptedModel> {
        self.review_with_model(ScriptedModel {
            response: Some(Ok(response(body))),
            mutation: None,
            request: None,
        })
    }

    fn review_with_model(&self, model: ScriptedModel) -> reviewer::ReviewRun<ScriptedModel> {
        Reviewer::new(
            &contract(),
            &handoff(),
            &self.manager,
            &self.worktree,
            &self.artifacts,
            model,
            "review-run",
            "reasoning",
        )
        .review(&sources(), &evidence())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.manager.cleanup(&self.worktree);
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn contract() -> TaskContract {
    TaskContract {
        id: text("id", "M2-011"),
        project_id: text("project_id", "noctis"),
        project_run_id: text("project_run_id", "run-1"),
        title: text("title", "Reviewer"),
        role: text("role", "reviewer"),
        objective: text("objective", "Review worker output"),
        depends_on: Vec::new(),
        allowed_paths: vec![AllowedPath::parse("src/**").unwrap()],
        context_refs: Vec::new(),
        acceptance_criteria: vec![
            text("acceptance_criteria", "Returns two typed decisions"),
            text("acceptance_criteria", "Cannot mutate worktree"),
        ],
        verification_commands: vec![text("verification_commands", "cargo test")],
        limits: TaskLimits {
            max_input_tokens: PositiveLimit::new("max_input_tokens", 10_000).unwrap(),
            max_output_tokens: PositiveLimit::new("max_output_tokens", 2_000).unwrap(),
            max_tool_calls: PositiveLimit::new("max_tool_calls", 1).unwrap(),
            max_attempts: MaxAttempts::new(1).unwrap(),
            timeout_seconds: PositiveLimit::new("timeout_seconds", 60).unwrap(),
        },
    }
}

fn handoff() -> WorkerHandoff {
    WorkerHandoff {
        summary: "Implemented reviewer".into(),
        next_status: Some(TaskStatus::Review),
        changed_paths: vec!["src/file.rs".into()],
        verification_commands: vec!["cargo test".into()],
        artifacts: vec!["verification".into()],
        token_usage: TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            tool_calls: 1,
        },
        stop_reason: StopReason::Completed,
    }
}

fn sources() -> Vec<SourceExcerpt> {
    vec![SourceExcerpt {
        path: "src/file.rs".into(),
        start_line: 1,
        content: "fn value() -> u8 { 2 }\n".into(),
    }]
}

fn evidence() -> Vec<VerificationEvidence> {
    vec![VerificationEvidence {
        command: "cargo test".into(),
        succeeded: true,
        artifact_id: Some("verification".into()),
    }]
}

fn response(body: &str) -> ModelResponse {
    ModelResponse {
        content: Some(body.into()),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
            cached_tokens: 0,
            total_tokens: 15,
            estimated: false,
        },
        latency_ms: 1,
    }
}

fn text(field: &'static str, value: &str) -> NonEmptyString {
    NonEmptyString::parse(field, value).unwrap()
}

#[test]
fn valid_approval_is_read_only_and_moves_to_verify() {
    let fixture = Fixture::new();
    let before = fs::read(fixture.worktree.path().join("src/file.rs")).unwrap();
    let run = fixture.review(r#"{"decision":"approved","findings":[]}"#);

    assert_eq!(run.error, None);
    let outcome = run.outcome.unwrap();
    assert_eq!(outcome.decision, ReviewDecision::Approved);
    assert_eq!(outcome.next_status, TaskStatus::Verify);
    assert_eq!(
        fs::read(fixture.worktree.path().join("src/file.rs")).unwrap(),
        before
    );
    let request = run.model.request.unwrap();
    assert!(request.tools.is_empty());
    assert!(request.messages[0].content.contains("all tests passed"));
    assert!(request.messages[0].content.contains("checksum"));
}

#[test]
fn missing_acceptance_criterion_requests_typed_change() {
    let fixture = Fixture::new();
    let run = fixture.review(
        r#"{"decision":"changes_requested","findings":[{"severity":"high","code":"MISSING_CRITERION","message":"Mutation guard lacks coverage","path":"src/file.rs","line":1}]}"#,
    );

    assert_eq!(run.error, None);
    let outcome = run.outcome.unwrap();
    assert_eq!(outcome.next_status, TaskStatus::ChangesRequested);
    assert!(matches!(
        outcome.decision,
        ReviewDecision::ChangesRequested { ref findings }
            if findings.len() == 1 && findings[0].code == "MISSING_CRITERION"
    ));
}

#[test]
fn path_violations_empty_findings_and_approved_findings_are_rejected() {
    let fixture = Fixture::new();
    for body in [
        r#"{"decision":"changes_requested","findings":[{"severity":"high","code":"BAD_PATH","message":"Invalid path","path":"../secret","line":1}]}"#,
        r#"{"decision":"changes_requested","findings":[]}"#,
        r#"{"decision":"approved","findings":[{"severity":"low","code":"EXTRA","message":"Unexpected finding","path":"src/file.rs","line":1}]}"#,
    ] {
        let run = fixture.review(body);
        assert!(matches!(
            run.error,
            Some(ReviewerError::InvalidFinding | ReviewerError::InvalidResponse)
        ));
        assert!(run.outcome.is_none());
    }
}

#[test]
fn oversized_response_and_finding_count_are_rejected() {
    let fixture = Fixture::new();
    let huge = format!(
        r#"{{"decision":"changes_requested","findings":[{{"severity":"high","code":"LARGE","message":"{}","path":"src/file.rs","line":1}}]}}"#,
        "x".repeat(70 * 1024)
    );
    assert_eq!(
        fixture.review(&huge).error,
        Some(ReviewerError::ResponseTooLarge)
    );

    let findings = (0..65)
        .map(|index| {
            format!(
                r#"{{"severity":"low","code":"F{index}","message":"Finding","path":"src/file.rs","line":1}}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let run = fixture.review(&format!(
        r#"{{"decision":"changes_requested","findings":[{findings}]}}"#
    ));
    assert_eq!(run.error, Some(ReviewerError::InvalidResponse));
}

#[test]
fn malformed_done_and_secret_body_fail_without_leaks() {
    let fixture = Fixture::new();
    let marker = "SECRET_MARKER /absolute/host/path";
    for body in [
        "not-json".to_owned(),
        r#"{"decision":"done","findings":[]}"#.to_owned(),
        format!(r#"{{"decision":"approved","findings":[],"secret":"{marker}"}}"#),
    ] {
        let run = fixture.review(&body);
        assert_eq!(run.error, Some(ReviewerError::InvalidResponse));
        let error = run.error.as_ref().unwrap();
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains(marker));
        assert!(!rendered.contains(fixture.root.to_string_lossy().as_ref()));
        assert!(!rendered.contains(&body));
    }
}

#[test]
fn attempted_mutation_is_policy_violation_even_with_approval() {
    let fixture = Fixture::new();
    let marker = "MUTATION_SECRET";
    let run = fixture.review_with_model(ScriptedModel {
        response: Some(Ok(response(r#"{"decision":"approved","findings":[]}"#))),
        mutation: Some(Mutation::Write(
            fixture.worktree.path().join("src/file.rs"),
            marker.into(),
        )),
        request: None,
    });

    assert_eq!(run.error, Some(ReviewerError::PolicyViolation));
    assert!(run.outcome.is_none());
    let error = run.error.as_ref().unwrap();
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains(marker));
    assert!(!rendered.contains(fixture.root.to_string_lossy().as_ref()));
}

#[test]
fn existing_untracked_content_change_is_detected_with_unchanged_status() {
    let fixture = Fixture::new();
    let path = fixture.worktree.path().join("src/untracked.txt");
    fs::write(&path, "same-size-a").unwrap();
    let run = fixture.review_with_model(ScriptedModel {
        response: Some(Ok(response(r#"{"decision":"approved","findings":[]}"#))),
        mutation: Some(Mutation::Write(path, "same-size-b".into())),
        request: None,
    });

    assert_eq!(run.error, Some(ReviewerError::PolicyViolation));
}

#[test]
fn deleted_and_new_untracked_files_are_detected() {
    let fixture = Fixture::new();
    let existing = fixture.worktree.path().join("src/existing.txt");
    fs::write(&existing, "existing").unwrap();
    let deleted = fixture.review_with_model(ScriptedModel {
        response: Some(Ok(response(r#"{"decision":"approved","findings":[]}"#))),
        mutation: Some(Mutation::Delete(existing)),
        request: None,
    });
    assert_eq!(deleted.error, Some(ReviewerError::PolicyViolation));

    let created = fixture.review_with_model(ScriptedModel {
        response: Some(Ok(response(r#"{"decision":"approved","findings":[]}"#))),
        mutation: Some(Mutation::Write(
            fixture.worktree.path().join("src/new.txt"),
            "new".into(),
        )),
        request: None,
    });
    assert_eq!(created.error, Some(ReviewerError::PolicyViolation));
}

#[cfg(unix)]
#[test]
fn untracked_symlink_is_rejected_without_exposing_target() {
    let fixture = Fixture::new();
    let target = fixture.root.join("SECRET_TARGET");
    fs::write(&target, "SECRET_CONTENT").unwrap();
    std::os::unix::fs::symlink(&target, fixture.worktree.path().join("src/link")).unwrap();

    let run = fixture.review(r#"{"decision":"approved","findings":[]}"#);
    assert_eq!(run.error, Some(ReviewerError::MutationInspection));
    let error = run.error.as_ref().unwrap();
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains("SECRET_TARGET"));
    assert!(!rendered.contains("SECRET_CONTENT"));
    assert!(!rendered.contains(fixture.root.to_string_lossy().as_ref()));
}

#[test]
fn oversized_and_excessive_untracked_files_are_typed_errors() {
    let fixture = Fixture::new();
    fs::write(
        fixture.worktree.path().join("src/large.bin"),
        vec![b'x'; 1024 * 1024 + 1],
    )
    .unwrap();
    let oversized = fixture.review(r#"{"decision":"approved","findings":[]}"#);
    assert_eq!(oversized.error, Some(ReviewerError::MutationInspection));

    fs::remove_file(fixture.worktree.path().join("src/large.bin")).unwrap();
    for index in 0..1025 {
        fs::write(
            fixture.worktree.path().join(format!("src/file-{index}")),
            "x",
        )
        .unwrap();
    }
    let excessive = fixture.review(r#"{"decision":"approved","findings":[]}"#);
    assert_eq!(excessive.error, Some(ReviewerError::MutationInspection));
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
