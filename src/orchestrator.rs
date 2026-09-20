use std::{
    env, fmt, fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    agent::{
        reviewer::{ReviewDecision, Reviewer, SourceExcerpt, VerificationEvidence},
        verifier::{ProductionExecutor, VerificationVerdict, Verifier},
        worker::{TokenUsage, Worker, WorkerConfig},
    },
    context::{ContextBuilder, ContextLimits, ContextRequest},
    domain::{
        state_machine::Actor,
        task::{NonEmptyString, TaskStatus},
    },
    openai::OpenAiToolsClient,
    runner::{
        container::ContainerLimits,
        git::{GitWorktreeManager, IntegrationResult},
        policy::{ToolPolicy, ToolRole},
        process::{ProcessRunner, VerificationCommand},
        tools::StructuredTools,
    },
    store::{
        artifact::ArtifactStore,
        event::{AttemptStatus, AttemptUpdate, ClaimAttempt, Usage},
        provider::ProviderRepository,
        task::{Conflict, StoreError, StoredTask, TaskRepository},
    },
};

#[derive(Clone, Debug)]
pub struct OrchestratorConfig {
    pub worktree_root: PathBuf,
    pub stale_after_seconds: i64,
    pub retention_lease_seconds: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrchestratorError {
    Store,
    Provider,
    Secret,
    Git,
    Context,
    Model,
    Worker,
    Reviewer,
    Verifier,
    Usage,
}

impl fmt::Display for OrchestratorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Store => "orchestrator persistence failed",
            Self::Provider => "orchestrator provider configuration is invalid",
            Self::Secret => "orchestrator provider secret is unavailable",
            Self::Git => "orchestrator git operation failed",
            Self::Context => "orchestrator context preparation failed",
            Self::Model => "orchestrator model setup failed",
            Self::Worker => "orchestrator worker failed",
            Self::Reviewer => "orchestrator reviewer failed",
            Self::Verifier => "orchestrator verification failed",
            Self::Usage => "orchestrator usage persistence failed",
        })
    }
}

impl std::error::Error for OrchestratorError {}

pub struct Orchestrator {
    tasks: TaskRepository,
    providers: ProviderRepository,
    artifacts: ArtifactStore,
    config: OrchestratorConfig,
}

impl Orchestrator {
    pub fn new(
        tasks: TaskRepository,
        providers: ProviderRepository,
        artifacts: ArtifactStore,
        config: OrchestratorConfig,
    ) -> Self {
        Self {
            tasks,
            providers,
            artifacts,
            config,
        }
    }

    pub async fn startup_recovery(&self) -> Result<usize, OrchestratorError> {
        let cutoff = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| OrchestratorError::Store)?
            .as_millis()
            .saturating_sub(self.config.stale_after_seconds.max(0) as u128 * 1_000)
            .try_into()
            .map_err(|_| OrchestratorError::Store)?;
        let mut recovered = 0;
        while self
            .tasks
            .recover_stale(cutoff)
            .await
            .map_err(|_| OrchestratorError::Store)?
            .is_some()
        {
            recovered += 1;
        }
        Ok(recovered)
    }

    pub async fn run(mut self, mut wake: mpsc::Receiver<()>) {
        loop {
            loop {
                match self.dispatch_once().await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(error) => {
                        eprintln!("{error}");
                        break;
                    }
                }
            }
            let _ = self.sweep_retention().await;
            tokio::select! {
                value = wake.recv() => if value.is_none() { break },
                () = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        }
    }

    pub async fn dispatch_once(&mut self) -> Result<bool, OrchestratorError> {
        let Some(attempt) = self
            .tasks
            .next_assigned()
            .await
            .map_err(|_| OrchestratorError::Store)?
        else {
            return Ok(false);
        };
        let dispatch_owner = match self.tasks.claim_dispatch(attempt.id).await {
            Ok(owner) => owner,
            Err(StoreError::Conflict(Conflict::Claim)) => return Ok(true),
            Err(_) => return Err(OrchestratorError::Store),
        };
        let attempt_id = attempt.id;
        if let Err(error) = self.execute(attempt, dispatch_owner).await {
            match self
                .tasks
                .fail_setup(attempt_id, dispatch_owner, error.code())
                .await
            {
                Ok(()) => {}
                Err(StoreError::Conflict(Conflict::Claim)) => self
                    .tasks
                    .fail_runtime(attempt_id, error.code())
                    .await
                    .map_err(|_| OrchestratorError::Store)?,
                Err(_) => return Err(OrchestratorError::Store),
            }
            eprintln!("{error}");
        }
        Ok(true)
    }

    pub async fn sweep_retention(&self) -> Result<bool, OrchestratorError> {
        let Some(claim) = self
            .tasks
            .claim_retention_due(self.config.retention_lease_seconds)
            .await
            .map_err(|_| OrchestratorError::Store)?
        else {
            return Ok(false);
        };
        let repository = self
            .tasks
            .project_repository(claim.attempt.task_id.as_str())
            .await
            .map_err(|_| OrchestratorError::Store)?;
        let git = GitWorktreeManager::new(repository, &self.config.worktree_root)
            .map_err(|_| OrchestratorError::Git)?;
        let worktree = match git.open(
            claim.attempt.task_id.as_str(),
            &claim.attempt.branch,
            &claim.attempt.base_commit,
        ) {
            Ok(worktree) => Some(worktree),
            Err(_)
                if !self
                    .config
                    .worktree_root
                    .join(claim.attempt.task_id.as_str())
                    .exists() =>
            {
                None
            }
            Err(_) => return Err(OrchestratorError::Git),
        };
        if let Some(worktree) = worktree {
            git.cleanup(&worktree).map_err(|_| OrchestratorError::Git)?;
        }
        self.tasks
            .complete_retention_cleanup(claim.attempt.id, claim.owner_token)
            .await
            .map_err(|_| OrchestratorError::Store)?;
        Ok(true)
    }

    async fn execute(
        &mut self,
        attempt: crate::store::event::RuntimeAttempt,
        dispatch_owner: Uuid,
    ) -> Result<(), OrchestratorError> {
        let task = self
            .tasks
            .get(attempt.task_id.as_str())
            .await
            .map_err(|_| OrchestratorError::Store)?;
        let model = self
            .providers
            .get_model(attempt.model_id.as_str())
            .await
            .map_err(|_| OrchestratorError::Provider)?;
        let provider = self
            .providers
            .get_provider(model.provider_id.as_str())
            .await
            .map_err(|_| OrchestratorError::Provider)?
            .provider;
        let secret =
            env::var(provider.api_key_env.as_str()).map_err(|_| OrchestratorError::Secret)?;
        let policy = ToolPolicy::new(
            ToolRole::Worker,
            task.contract
                .allowed_paths
                .iter()
                .map(|path| path.as_str().to_owned())
                .collect(),
            64 * 1024,
            1024 * 1024,
            Duration::from_secs(task.contract.limits.timeout_seconds.get() as u64),
        )
        .map_err(|_| OrchestratorError::Context)?;
        let client = OpenAiToolsClient::new(
            provider.base_url.as_str(),
            secret,
            model.remote_name.as_str(),
            Duration::from_secs(provider.request_timeout_seconds.get() as u64),
        )
        .map_err(|_| OrchestratorError::Model)?;
        let repository = self
            .tasks
            .project_repository(attempt.task_id.as_str())
            .await
            .map_err(|_| OrchestratorError::Store)?;
        let git = GitWorktreeManager::new(&repository, &self.config.worktree_root)
            .map_err(|_| OrchestratorError::Git)?;
        let worktree = git
            .create(
                attempt.task_id.as_str(),
                &attempt.branch,
                &attempt.base_commit,
            )
            .or_else(|_| {
                git.open(
                    attempt.task_id.as_str(),
                    &attempt.branch,
                    &attempt.base_commit,
                )
            })
            .map_err(|_| OrchestratorError::Git)?;
        let context = match ContextBuilder::new(
            worktree.path(),
            &self.artifacts,
            ContextLimits {
                max_bytes: 4 * 1024 * 1024,
                max_tokens: task.contract.limits.max_input_tokens.get() as usize,
                max_file_bytes: 1024 * 1024,
                max_command_output_bytes: 1024 * 1024,
            },
        ) {
            Ok(context) => context,
            Err(_) => {
                git.cleanup(&worktree).map_err(|_| OrchestratorError::Git)?;
                return Err(OrchestratorError::Context);
            }
        };
        if context
            .build(&task.contract, &ContextRequest::default())
            .is_err()
        {
            git.cleanup(&worktree).map_err(|_| OrchestratorError::Git)?;
            return Err(OrchestratorError::Context);
        }
        let tools = StructuredTools::new(policy, &git, &worktree, &self.artifacts);
        if self
            .tasks
            .start_claimed(attempt.id, dispatch_owner)
            .await
            .is_err()
        {
            drop(tools);
            drop(context);
            git.cleanup(&worktree).map_err(|_| OrchestratorError::Git)?;
            return Err(OrchestratorError::Store);
        }
        let task = self
            .tasks
            .get(attempt.task_id.as_str())
            .await
            .map_err(|_| OrchestratorError::Store)?;
        let run = Worker::new(
            &task.contract,
            TaskStatus::Running,
            &context,
            &tools,
            client,
            self.tasks.clone(),
            WorkerConfig {
                agent_run_id: attempt.id.to_string(),
                model_class: model.class.as_str().to_owned(),
                max_turns: 32,
                deadline: Duration::from_secs(task.contract.limits.timeout_seconds.get() as u64),
            },
        )
        .run()
        .await;
        self.record_usage(attempt.id, "worker", run.handoff.token_usage)
            .await?;
        if run.error.is_some() || run.handoff.next_status != Some(TaskStatus::SelfCheck) {
            return self.fail_attempt(attempt.id, "worker.failed").await;
        }
        let current = self
            .transition(&task, TaskStatus::SelfCheck, Actor::Worker)
            .await?;
        let current = self
            .transition(&current, TaskStatus::Review, Actor::Worker)
            .await?;
        let review = Reviewer::new(
            &current.contract,
            &run.handoff,
            &git,
            &worktree,
            &self.artifacts,
            run.model,
            attempt.id.to_string(),
            model.class.as_str(),
        )
        .review(&[] as &[SourceExcerpt], &[] as &[VerificationEvidence])
        .await;
        self.record_usage(attempt.id, "reviewer", review.usage)
            .await?;
        let outcome = review.outcome.ok_or(OrchestratorError::Reviewer)?;
        let current = self
            .transition(&current, outcome.next_status, Actor::Reviewer)
            .await?;
        if matches!(outcome.decision, ReviewDecision::ChangesRequested { .. }) {
            let _ = self
                .transition(&current, TaskStatus::Ready, Actor::System)
                .await?;
            return self
                .close_failed_attempt(attempt.id, "review.changes_requested")
                .await;
        }
        let commands = verification_commands(&current)?;
        let runner = ProcessRunner::new(
            worktree.path(),
            env::var("NOCTIS_RUNNER_IMAGE").unwrap_or_else(|_| "rust:1".to_owned()),
            commands,
            Vec::new(),
            Duration::from_secs(current.contract.limits.timeout_seconds.get() as u64),
            1024 * 1024,
            ContainerLimits {
                cpu_count: "1".to_owned(),
                memory_bytes: 1024 * 1024 * 1024,
                process_limit: 256,
            },
        )
        .map_err(|_| OrchestratorError::Verifier)?;
        let verifier = Verifier::new(
            &current.contract,
            TaskStatus::Verify,
            worktree.path(),
            ProductionExecutor::new(&runner, &self.artifacts),
        )
        .map_err(|_| OrchestratorError::Verifier)?;
        let report = verifier.verify().map_err(|_| OrchestratorError::Verifier)?;
        let current = self
            .transition(&current, report.proposed_status, Actor::Verifier)
            .await?;
        if report.verdict == VerificationVerdict::Failed {
            return self
                .close_failed_attempt(attempt.id, "verification.failed")
                .await;
        }
        let target_id = format!("{}-integration", attempt.task_id.as_str());
        let target_branch = format!("integration-{}", attempt.task_id.as_str());
        let target = git
            .create(&target_id, &target_branch, &attempt.base_commit)
            .or_else(|_| git.open(&target_id, &target_branch, &attempt.base_commit))
            .map_err(|_| OrchestratorError::Git)?;
        let integrated = git
            .integrate_verified(&worktree, &target)
            .map_err(|_| OrchestratorError::Git)?;
        let terminal = match integrated {
            IntegrationResult::Integrated => TaskStatus::Done,
            IntegrationResult::Conflict => TaskStatus::Conflict,
        };
        self.transition(&current, terminal, Actor::Integrator)
            .await?;
        self.tasks
            .update_attempt(
                attempt.id,
                &AttemptUpdate {
                    status: if terminal == TaskStatus::Done {
                        AttemptStatus::Completed
                    } else {
                        AttemptStatus::Failed
                    },
                    error_code: (terminal == TaskStatus::Conflict)
                        .then(|| "integration.conflict".to_owned()),
                },
            )
            .await
            .map_err(|_| OrchestratorError::Store)?;
        Ok(())
    }

    async fn transition(
        &self,
        task: &StoredTask,
        to: TaskStatus,
        actor: Actor,
    ) -> Result<StoredTask, OrchestratorError> {
        self.tasks
            .transition(task.contract.id.as_str(), task.version, to, actor)
            .await
            .map_err(|_| OrchestratorError::Store)
    }

    async fn record_usage(
        &self,
        attempt_id: Uuid,
        operation: &str,
        usage: TokenUsage,
    ) -> Result<(), OrchestratorError> {
        let usage = Usage {
            input_tokens: usage
                .input_tokens
                .try_into()
                .map_err(|_| OrchestratorError::Usage)?,
            cached_tokens: usage
                .cached_tokens
                .try_into()
                .map_err(|_| OrchestratorError::Usage)?,
            output_tokens: usage
                .output_tokens
                .try_into()
                .map_err(|_| OrchestratorError::Usage)?,
            tool_calls: usage
                .tool_calls
                .try_into()
                .map_err(|_| OrchestratorError::Usage)?,
            latency_ms: usage
                .latency_ms
                .try_into()
                .map_err(|_| OrchestratorError::Usage)?,
            estimated: usage.estimated,
        };
        self.tasks
            .record_usage_once(attempt_id, operation, &usage)
            .await
            .map_err(|_| OrchestratorError::Usage)?;
        Ok(())
    }

    async fn fail_attempt(
        &self,
        attempt_id: Uuid,
        error_code: &str,
    ) -> Result<(), OrchestratorError> {
        self.tasks
            .fail_runtime(attempt_id, error_code)
            .await
            .map_err(|_| OrchestratorError::Store)?;
        Ok(())
    }

    async fn close_failed_attempt(
        &self,
        attempt_id: Uuid,
        error_code: &str,
    ) -> Result<(), OrchestratorError> {
        self.tasks
            .close_failed_attempt(attempt_id, error_code)
            .await
            .map_err(|_| OrchestratorError::Store)
    }
}

impl OrchestratorError {
    fn code(self) -> &'static str {
        match self {
            Self::Store => "orchestrator.store",
            Self::Provider => "orchestrator.provider",
            Self::Secret => "orchestrator.secret",
            Self::Git => "orchestrator.git",
            Self::Context => "orchestrator.context",
            Self::Model => "orchestrator.model",
            Self::Worker => "orchestrator.worker",
            Self::Reviewer => "orchestrator.reviewer",
            Self::Verifier => "orchestrator.verifier",
            Self::Usage => "orchestrator.usage",
        }
    }
}

fn verification_commands(task: &StoredTask) -> Result<Vec<VerificationCommand>, OrchestratorError> {
    task.contract
        .verification_commands
        .iter()
        .map(|raw| {
            let mut parts = raw.as_str().split_ascii_whitespace();
            let executable = parts.next().ok_or(OrchestratorError::Verifier)?;
            Ok(VerificationCommand {
                class: "verification".to_owned(),
                executable: executable.to_owned(),
                arguments: parts.map(str::to_owned).collect(),
            })
        })
        .collect()
}

pub struct StartRequest<'a> {
    pub task_id: &'a str,
    pub expected_version: i64,
    pub model_id: &'a str,
    pub retention_seconds: i64,
    pub attempt_id: Uuid,
}

pub async fn claim_task(
    tasks: &TaskRepository,
    providers: &ProviderRepository,
    request: StartRequest<'_>,
) -> Result<StoredTask, StoreError> {
    let task = tasks.get(request.task_id).await?;
    let model = providers
        .get_model(request.model_id)
        .await
        .map_err(|_| StoreError::InvalidId("model_id"))?;
    let repository = tasks.project_repository(request.task_id).await?;
    let base_commit =
        repository_head(Path::new(&repository)).ok_or(StoreError::InvalidId("base_commit"))?;
    let claim = ClaimAttempt {
        id: request.attempt_id,
        task_id: task.contract.id.clone(),
        role: task.contract.role.clone(),
        provider_id: NonEmptyString::parse("provider_id", model.provider_id.as_str())?,
        model_id: NonEmptyString::parse("model_id", model.id.as_str())?,
        branch: format!("noctis-{}-{}", request.task_id, request.attempt_id.simple()),
        base_commit,
        retention_seconds: request.retention_seconds,
    };
    tasks.claim_ready(request.expected_version, &claim).await?;
    tasks.get(request.task_id).await
}

fn repository_head(repository: &Path) -> Option<String> {
    let git = git_directory(repository)?;
    let head = fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    if valid_commit(head) {
        return Some(head.to_owned());
    }
    let reference = head.strip_prefix("ref: ")?;
    let loose = git.join(reference);
    if let Ok(commit) = fs::read_to_string(loose) {
        let commit = commit.trim();
        if valid_commit(commit) {
            return Some(commit.to_owned());
        }
    }
    fs::read_to_string(git.join("packed-refs"))
        .ok()?
        .lines()
        .filter(|line| !line.starts_with(['#', '^']))
        .find_map(|line| {
            let (commit, name) = line.split_once(' ')?;
            (name == reference && valid_commit(commit)).then(|| commit.to_owned())
        })
}

fn git_directory(repository: &Path) -> Option<PathBuf> {
    let dot_git = repository.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let pointer = fs::read_to_string(dot_git).ok()?;
    let path = pointer.trim().strip_prefix("gitdir: ")?;
    Some(if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        repository.join(path)
    })
}

fn valid_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::valid_commit;

    #[test]
    fn commit_validation_rejects_symbolic_and_malformed_values() {
        assert!(valid_commit("0123456789012345678901234567890123456789"));
        assert!(!valid_commit("HEAD"));
        assert!(!valid_commit("secret-value"));
    }
}
