use std::{
    env, fmt,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use uuid::Uuid;

#[path = "scheduler/mod.rs"]
pub mod scheduler;
pub use scheduler::SequentialScheduler;

use crate::{
    agent::{
        reviewer::{ReviewDecision, Reviewer, SourceExcerpt, VerificationEvidence},
        verifier::{ProductionExecutor, VerificationVerdict, Verifier},
        worker::{TokenUsage, Worker, WorkerConfig},
    },
    context::{ContextBuilder, ContextLimits, ContextRequest},
    domain::{
        provider::VerifiedCapability,
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
        event::{
            ArtifactRecord, ClaimAttempt, DurableEvent, IntegrationOperation, IntegrationStatus,
            Usage,
        },
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
    Deadline,
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
            Self::Deadline => "orchestrator task deadline exceeded",
        })
    }
}

impl std::error::Error for OrchestratorError {}

/// Mencatat penyebab asli lalu memetakannya ke kategori `Store`. Dulu penyebab dibuang
/// (`map_err(|_| ..)`), sehingga operator hanya melihat "persistence failed" tanpa petunjuk.
fn store_error<E: fmt::Display>(error: E) -> OrchestratorError {
    tracing::error!(cause = %error, "orchestrator persistence failed");
    OrchestratorError::Store
}

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
            .map_err(store_error)?
            .as_millis()
            .saturating_sub(self.config.stale_after_seconds.max(0) as u128 * 1_000)
            .try_into()
            .map_err(store_error)?;
        let mut recovered = self.recover_integrations().await?;
        while let Some(attempt) = self.tasks.next_stale(cutoff).await.map_err(store_error)? {
            let repository = self
                .tasks
                .project_repository(attempt.task_id.as_str())
                .await
                .map_err(store_error)?;
            let force_human = match GitWorktreeManager::new(repository, &self.config.worktree_root)
            {
                Ok(git) => match git.open(
                    attempt.task_id.as_str(),
                    &attempt.branch,
                    &attempt.base_commit,
                ) {
                    Ok(worktree) => git.cleanup(&worktree).is_err(),
                    Err(_) => true,
                },
                Err(_) => true,
            };
            self.tasks
                .recover_stale_attempt(attempt.id, force_human)
                .await
                .map_err(store_error)?;
            recovered += 1;
        }
        Ok(recovered)
    }

    async fn recover_integrations(&self) -> Result<usize, OrchestratorError> {
        let mut recovered = 0;
        for operation in self
            .tasks
            .pending_integrations()
            .await
            .map_err(store_error)?
        {
            let task = self
                .tasks
                .get(operation.task_id.as_str())
                .await
                .map_err(store_error)?;
            if task.status == TaskStatus::Done && operation.status == IntegrationStatus::Applied {
                self.tasks
                    .finalize_integration(
                        operation.id,
                        operation.owner_token,
                        IntegrationStatus::Applied,
                        TaskStatus::Done,
                    )
                    .await
                    .map_err(store_error)?;
                recovered += 1;
                continue;
            }
            if task.status == TaskStatus::Conflict
                && operation.status == IntegrationStatus::Conflict
            {
                self.tasks
                    .finalize_integration(
                        operation.id,
                        operation.owner_token,
                        IntegrationStatus::Conflict,
                        TaskStatus::Conflict,
                    )
                    .await
                    .map_err(store_error)?;
                recovered += 1;
                continue;
            }
            if task.status != TaskStatus::Integrate {
                continue;
            }
            if operation.status == IntegrationStatus::Conflict {
                self.tasks
                    .finalize_integration(
                        operation.id,
                        operation.owner_token,
                        IntegrationStatus::Conflict,
                        TaskStatus::Conflict,
                    )
                    .await
                    .map_err(store_error)?;
                recovered += 1;
                continue;
            }
            let repository = self
                .tasks
                .project_repository(operation.task_id.as_str())
                .await
                .map_err(store_error)?;
            let Ok(git) = GitWorktreeManager::new(repository, &self.config.worktree_root) else {
                self.tasks
                    .finalize_integration(
                        operation.id,
                        operation.owner_token,
                        operation.status,
                        TaskStatus::NeedsHuman,
                    )
                    .await
                    .map_err(store_error)?;
                recovered += 1;
                continue;
            };
            let source = git.open(
                operation.task_id.as_str(),
                &operation.source_branch,
                &operation.source_base_commit,
            );
            let target = git.open(
                &operation.target_id,
                &operation.target_branch,
                &operation.target_base_commit,
            );
            let outcome = match (source, target) {
                (Ok(source), Ok(target)) => {
                    match (git.diff_binary(&source), git.diff_binary(&target)) {
                        (Ok(source_patch), Ok(target_patch)) => {
                            let source_hash = format!("{:x}", Sha256::digest(&source_patch));
                            let target_hash = format!("{:x}", Sha256::digest(&target_patch));
                            if let Some(status) = recovered_integration_status(
                                operation.status,
                                &target_hash,
                                &operation.patch_sha256,
                            ) {
                                (status, operation.status)
                            } else if operation.status == IntegrationStatus::Prepared
                                && target_patch.is_empty()
                                && source_hash == operation.patch_sha256
                            {
                                match git.integrate_verified(&source, &target) {
                                    Ok(IntegrationResult::Integrated) => {
                                        (TaskStatus::Done, IntegrationStatus::Prepared)
                                    }
                                    Ok(IntegrationResult::Conflict) => {
                                        (TaskStatus::Conflict, IntegrationStatus::Prepared)
                                    }
                                    Err(_) => (TaskStatus::NeedsHuman, operation.status),
                                }
                            } else {
                                (TaskStatus::NeedsHuman, operation.status)
                            }
                        }
                        _ => (TaskStatus::NeedsHuman, operation.status),
                    }
                }
                _ => (TaskStatus::NeedsHuman, operation.status),
            };
            self.tasks
                .finalize_integration(operation.id, operation.owner_token, outcome.1, outcome.0)
                .await
                .map_err(store_error)?;
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
                        tracing::error!(%error, "dispatch orchestrator gagal");
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
        let Some(attempt) = self.tasks.next_assigned().await.map_err(store_error)? else {
            return Ok(false);
        };
        self.dispatch_claimed(attempt).await
    }

    async fn dispatch_claimed(
        &mut self,
        attempt: crate::store::event::RuntimeAttempt,
    ) -> Result<bool, OrchestratorError> {
        let dispatch_owner = match self.tasks.claim_dispatch(attempt.id).await {
            Ok(owner) => owner,
            Err(StoreError::Conflict(Conflict::Claim)) => return Ok(true),
            Err(error) => return Err(store_error(error)),
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
                    .map_err(store_error)?,
                Err(error) => return Err(store_error(error)),
            }
            tracing::error!(%attempt_id, %error, "dispatch attempt gagal");
        }
        Ok(true)
    }

    pub async fn run_sequential(
        &mut self,
        scheduler: &SequentialScheduler,
        mut wake: mpsc::Receiver<()>,
    ) {
        loop {
            loop {
                match self.dispatch_once().await {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => {
                        tracing::error!(%error, "dispatch orchestrator gagal");
                        break;
                    }
                }
                match self.dispatch_sequential_once(scheduler).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(error) => {
                        tracing::error!(%error, "dispatch orchestrator gagal");
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

    pub async fn dispatch_sequential_once(
        &mut self,
        scheduler: &SequentialScheduler,
    ) -> Result<bool, OrchestratorError> {
        let Some(attempt) = scheduler.claim_one().await.map_err(store_error)? else {
            return Ok(false);
        };
        self.dispatch_claimed(attempt).await
    }

    pub async fn sweep_retention(&self) -> Result<bool, OrchestratorError> {
        let Some(claim) = self
            .tasks
            .claim_retention_due(self.config.retention_lease_seconds)
            .await
            .map_err(store_error)?
        else {
            return Ok(false);
        };
        let repository = self
            .tasks
            .project_repository(claim.attempt.task_id.as_str())
            .await
            .map_err(store_error)?;
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
            .map_err(store_error)?;
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
            .map_err(store_error)?;
        let deadline =
            Instant::now() + Duration::from_secs(task.contract.limits.timeout_seconds.get() as u64);
        let model = self
            .providers
            .get_model(attempt.model_id.as_str())
            .await
            .map_err(|_| OrchestratorError::Provider)?;
        if !model.capabilities.claimed.tools
            || model.capabilities.verified.tools != VerifiedCapability::Supported
        {
            return Err(OrchestratorError::Model);
        }
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
            .map_err(store_error)?;
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
        if let Err(error) = self.tasks.start_claimed(attempt.id, dispatch_owner).await {
            drop(tools);
            drop(context);
            git.cleanup(&worktree).map_err(|_| OrchestratorError::Git)?;
            return Err(store_error(error));
        }
        let task = self
            .tasks
            .get(attempt.task_id.as_str())
            .await
            .map_err(store_error)?;
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
                deadline: deadline.saturating_duration_since(Instant::now()),
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
        );
        let review = tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            review.review(&[] as &[SourceExcerpt], &[] as &[VerificationEvidence]),
        )
        .await
        .map_err(|_| OrchestratorError::Deadline)?;
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
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(OrchestratorError::Deadline);
        }
        let commands = verification_commands(&current)?;
        let runner = ProcessRunner::new(
            worktree.path(),
            env::var("NOCTIS_RUNNER_IMAGE").unwrap_or_else(|_| "rust:1".to_owned()),
            commands,
            Vec::new(),
            remaining,
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
        for (index, result) in report.results.iter().enumerate() {
            self.tasks
                .record_event_once(
                    attempt.task_id.as_str(),
                    &format!("{}-verification-{index}", attempt.id),
                    &DurableEvent::Verification {
                        passed: result.passed,
                        exit_code: result.exit_code,
                        timed_out: result.timed_out,
                        command_index: index,
                        artifact_ids: [
                            result.stdout_artifact_id.clone(),
                            result.stderr_artifact_id.clone(),
                        ],
                    },
                )
                .await
                .map_err(store_error)?;
        }
        self.transition(&current, report.proposed_status, Actor::Verifier)
            .await?;
        if deadline <= Instant::now() {
            return Err(OrchestratorError::Deadline);
        }
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
        let patch = git
            .diff_binary(&worktree)
            .map_err(|_| OrchestratorError::Git)?;
        let patch_sha256 = format!("{:x}", Sha256::digest(&patch));
        self.persist_diff(attempt.task_id.as_str(), attempt.id, &patch)
            .await?;
        let operation = self
            .tasks
            .prepare_integration(&IntegrationOperation {
                id: Uuid::new_v4(),
                attempt_id: attempt.id,
                task_id: attempt.task_id.clone(),
                source_branch: attempt.branch.clone(),
                source_base_commit: attempt.base_commit.clone(),
                target_id: target_id.clone(),
                target_branch: target_branch.clone(),
                target_base_commit: attempt.base_commit.clone(),
                patch_sha256,
                owner_token: Uuid::new_v4(),
                status: IntegrationStatus::Prepared,
            })
            .await
            .map_err(store_error)?;
        let integrated = git
            .integrate_verified(&worktree, &target)
            .map_err(|_| OrchestratorError::Git)?;
        let terminal = match integrated {
            IntegrationResult::Integrated => TaskStatus::Done,
            IntegrationResult::Conflict => TaskStatus::Conflict,
        };
        self.tasks
            .finalize_integration(
                operation.id,
                operation.owner_token,
                IntegrationStatus::Prepared,
                terminal,
            )
            .await
            .map_err(store_error)?;
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
            .map_err(store_error)
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
        let attempt = self
            .tasks
            .get_attempt(attempt_id)
            .await
            .map_err(|_| OrchestratorError::Usage)?;
        self.tasks
            .record_event_once(
                attempt.task_id.as_str(),
                &format!("{attempt_id}-usage-{operation}"),
                &DurableEvent::Usage {
                    input_tokens: usage.input_tokens,
                    cached_tokens: usage.cached_tokens,
                    output_tokens: usage.output_tokens,
                    tool_calls: usage.tool_calls,
                    latency_ms: usage.latency_ms,
                    estimated: usage.estimated,
                },
            )
            .await
            .map_err(|_| OrchestratorError::Usage)?;
        Ok(())
    }

    async fn persist_diff(
        &self,
        task_id: &str,
        attempt_id: Uuid,
        patch: &[u8],
    ) -> Result<(), OrchestratorError> {
        let artifact_id = attempt_id;
        let checksum = format!("{:x}", Sha256::digest(patch));
        let created = match self.artifacts.write(
            &artifact_id.to_string(),
            "official.diff",
            "text/x-diff",
            patch,
            |_| Ok::<_, ()>(()),
        ) {
            Ok(_) => true,
            Err(crate::store::artifact::ArtifactError::AlreadyExists) => {
                let existing = self
                    .artifacts
                    .read(&artifact_id.to_string())
                    .map_err(store_error)?;
                if format!("{:x}", Sha256::digest(existing)) != checksum {
                    tracing::error!(%artifact_id, "artifact sudah ada dengan checksum berbeda");
                    return Err(OrchestratorError::Store);
                }
                false
            }
            Err(error) => return Err(store_error(error)),
        };
        let result = self
            .tasks
            .persist_artifact_once(&ArtifactRecord {
                id: artifact_id,
                task_id: NonEmptyString::parse("task_id", task_id).map_err(store_error)?,
                kind: NonEmptyString::parse("kind", "diff").map_err(store_error)?,
                logical_name: NonEmptyString::parse("logical_name", "official.diff")
                    .map_err(store_error)?,
                media_type: NonEmptyString::parse("media_type", "text/x-diff")
                    .map_err(store_error)?,
                size: patch.len().try_into().map_err(store_error)?,
                checksum,
            })
            .await;
        if result.is_err() && created {
            let _ = self.artifacts.remove(&artifact_id.to_string());
        }
        result.map_err(store_error)
    }

    async fn fail_attempt(
        &self,
        attempt_id: Uuid,
        error_code: &str,
    ) -> Result<(), OrchestratorError> {
        self.tasks
            .fail_runtime(attempt_id, error_code)
            .await
            .map_err(store_error)?;
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
            .map_err(store_error)
    }
}

fn recovered_integration_status(
    checkpoint: IntegrationStatus,
    target_hash: &str,
    patch_hash: &str,
) -> Option<TaskStatus> {
    match checkpoint {
        IntegrationStatus::Conflict => Some(TaskStatus::Conflict),
        IntegrationStatus::Prepared | IntegrationStatus::Applied if target_hash == patch_hash => {
            Some(TaskStatus::Done)
        }
        IntegrationStatus::Applied => Some(TaskStatus::NeedsHuman),
        IntegrationStatus::Prepared => None,
        IntegrationStatus::Completed => Some(TaskStatus::NeedsHuman),
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
            Self::Deadline => "orchestrator.deadline",
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
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["rev-parse", "--verify", "HEAD"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let commit = std::str::from_utf8(&output.stdout).ok()?.trim();
    (output.status.success() && valid_commit(commit)).then(|| commit.to_owned())
}

fn valid_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_validation_rejects_symbolic_and_malformed_values() {
        assert!(valid_commit("0123456789012345678901234567890123456789"));
        assert!(!valid_commit("HEAD"));
        assert!(!valid_commit("secret-value"));
    }

    #[test]
    fn recovery_recognizes_applied_prepared_checkpoint_and_rejects_other_fingerprints() {
        assert_eq!(
            recovered_integration_status(IntegrationStatus::Prepared, "patch", "patch"),
            Some(TaskStatus::Done)
        );
        assert_eq!(
            recovered_integration_status(IntegrationStatus::Prepared, "other", "patch"),
            None
        );
        assert_eq!(
            recovered_integration_status(IntegrationStatus::Applied, "other", "patch"),
            Some(TaskStatus::NeedsHuman)
        );
        assert_eq!(
            recovered_integration_status(IntegrationStatus::Conflict, "other", "patch"),
            Some(TaskStatus::Conflict)
        );
    }
}
