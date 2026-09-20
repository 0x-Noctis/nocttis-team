use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};

use ai_team::{
    domain::{
        state_machine::Actor,
        task::{
            AllowedPath, MaxAttempts, NonEmptyString, PositiveLimit, TaskContract, TaskLimits,
            TaskStatus,
        },
    },
    store::{
        artifact::ArtifactStore,
        event::{
            ArtifactRecord, AttemptStatus, AttemptUpdate, ClaimAttempt, DispatchClaim,
            RecoveryDisposition, RuntimeAttempt, ToolCallMetadata, ToolCallReservation,
            ToolOutcome, Usage,
        },
        task::{Conflict, StoreError, TaskRepository},
    },
};
use sqlx::{PgPool, Row};
use uuid::Uuid;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

async fn ownership(pool: &PgPool) -> (Uuid, Uuid) {
    let project_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO projects (id,name,repository_path) VALUES ($1,'runtime','/repository')",
    )
    .bind(project_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'runtime','running',100)")
        .bind(run_id).bind(project_id).execute(pool).await.unwrap();
    (project_id, run_id)
}

fn text(field: &'static str, value: impl Into<String>) -> NonEmptyString {
    NonEmptyString::parse(field, value).unwrap()
}

fn contract(id: &str, project_id: Uuid, run_id: Uuid, max_attempts: i64) -> TaskContract {
    TaskContract {
        id: text("id", id),
        project_id: text("project_id", project_id.to_string()),
        project_run_id: text("project_run_id", run_id.to_string()),
        title: text("title", "Runtime"),
        role: text("role", "worker"),
        objective: text("objective", "Recover runtime"),
        depends_on: vec![],
        allowed_paths: vec![AllowedPath::parse("src/**").unwrap()],
        context_refs: vec![],
        acceptance_criteria: vec![text("acceptance_criteria", "works")],
        verification_commands: vec![text("verification_commands", "cargo test")],
        limits: TaskLimits {
            max_input_tokens: PositiveLimit::new("max_input_tokens", 100).unwrap(),
            max_output_tokens: PositiveLimit::new("max_output_tokens", 100).unwrap(),
            max_tool_calls: PositiveLimit::new("max_tool_calls", 10).unwrap(),
            max_attempts: MaxAttempts::new(max_attempts).unwrap(),
            timeout_seconds: PositiveLimit::new("timeout_seconds", 60).unwrap(),
        },
    }
}

async fn ready(repository: &TaskRepository, task: &TaskContract) {
    repository.create(task).await.unwrap();
    repository
        .transition(task.id.as_str(), 0, TaskStatus::Planned, Actor::System)
        .await
        .unwrap();
    repository
        .transition(task.id.as_str(), 1, TaskStatus::Ready, Actor::System)
        .await
        .unwrap();
}

fn claim(task_id: &str) -> ClaimAttempt {
    ClaimAttempt {
        id: Uuid::new_v4(),
        task_id: text("task_id", task_id),
        role: text("role", "worker"),
        provider_id: text("provider_id", "provider"),
        model_id: text("model_id", "model"),
        branch: "runtime-branch".to_owned(),
        base_commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
        retention_seconds: 3600,
    }
}

fn dispatch(id: Uuid) -> DispatchClaim {
    DispatchClaim {
        id,
        role: text("role", "worker"),
        provider_id: text("provider_id", "provider"),
        model_id: text("model_id", "registry-model-id"),
        branch: "runtime-branch".to_owned(),
        base_commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
        retention_seconds: 3600,
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_ready_claim_has_one_winner(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    let task = contract("claim-race", project_id, run_id, 2);
    ready(&repository, &task).await;
    let left = claim(task.id.as_str());
    let right = claim(task.id.as_str());

    let (left_result, right_result) = tokio::join!(
        repository.claim_ready(2, &left),
        repository.claim_ready(2, &right)
    );
    assert!(matches!(
        (&left_result, &right_result),
        (Ok(_), Err(StoreError::Conflict(Conflict::Claim)))
            | (Err(StoreError::Conflict(Conflict::Claim)), Ok(_))
    ));
    assert_eq!(
        repository.get(task.id.as_str()).await.unwrap().status,
        TaskStatus::Assigned
    );
    assert_eq!(
        repository
            .list_attempts(task.id.as_str())
            .await
            .unwrap()
            .len(),
        1
    );
    let events = repository.events(task.id.as_str()).await.unwrap();
    assert_eq!(events.last().unwrap().to_status, Some(TaskStatus::Assigned));
}

#[sqlx::test(migrations = "./migrations")]
async fn attempt_recovery_roundtrip_and_sanitized_update(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    let task = contract("recovery", project_id, run_id, 2);
    ready(&repository, &task).await;
    let claimed = repository
        .claim_ready(2, &claim(task.id.as_str()))
        .await
        .unwrap();
    assert_eq!(repository.get_attempt(claimed.id).await.unwrap(), claimed);
    assert_eq!(
        repository.unfinished_attempts().await.unwrap(),
        vec![claimed.clone()]
    );

    let updated = repository
        .update_attempt(
            claimed.id,
            &AttemptUpdate {
                status: AttemptStatus::Failed,
                error_code: Some("timeout.model".to_owned()),
            },
        )
        .await
        .unwrap();
    assert!(updated.finished_unix_ms.is_some());
    assert!(repository.unfinished_attempts().await.unwrap().is_empty());
    let unsafe_code = "secret/path?token=abc";
    let error = repository
        .update_attempt(
            claimed.id,
            &AttemptUpdate {
                status: AttemptStatus::Failed,
                error_code: Some(unsafe_code.to_owned()),
            },
        )
        .await
        .unwrap_err();
    assert!(!format!("{error} {error:?}").contains(unsafe_code));
    let stored_code: Option<String> =
        sqlx::query_scalar("SELECT error_code FROM agent_runs WHERE id=$1")
            .bind(claimed.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored_code.as_deref(), Some("timeout.model"));
}

#[sqlx::test(migrations = "./migrations")]
async fn runtime_attempt_create_get_list_and_retention_roundtrip(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool);
    let task = contract("attempt-crud", project_id, run_id, 2);
    repository.create(&task).await.unwrap();
    let attempt = RuntimeAttempt {
        id: Uuid::new_v4(),
        task_id: task.id.clone(),
        role: text("role", "worker"),
        provider_id: text("provider_id", "provider"),
        model_id: text("model_id", "model"),
        attempt: 1,
        status: AttemptStatus::Running,
        branch: "attempt-branch".to_owned(),
        base_commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
        heartbeat_unix_ms: None,
        finished_unix_ms: None,
        retain_until_unix_ms: None,
        error_code: None,
    };

    repository.create_runtime_attempt(&attempt).await.unwrap();
    assert_eq!(repository.get_attempt(attempt.id).await.unwrap(), attempt);
    assert_eq!(
        repository.list_attempts(task.id.as_str()).await.unwrap(),
        vec![attempt]
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn secret_and_path_markers_are_rejected_without_persistence(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    let task = contract("redaction", project_id, run_id, 1);
    ready(&repository, &task).await;
    let mut unsafe_claim = claim(task.id.as_str());
    unsafe_claim.branch = "secret/path-marker".to_owned();
    assert!(repository.claim_ready(2, &unsafe_claim).await.is_err());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM agent_runs WHERE task_id=$1")
        .bind(task.id.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        repository.get(task.id.as_str()).await.unwrap().status,
        TaskStatus::Ready
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn tool_reservation_is_new_in_progress_completed_and_idempotent(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool);
    let task = contract("tool", project_id, run_id, 2);
    ready(&repository, &task).await;
    let attempt = repository
        .claim_ready(2, &claim(task.id.as_str()))
        .await
        .unwrap();

    assert_eq!(
        repository
            .reserve_tool_call(attempt.id, "call-1")
            .await
            .unwrap(),
        ToolCallReservation::New
    );
    assert_eq!(
        repository
            .reserve_tool_call(attempt.id, "call-1")
            .await
            .unwrap(),
        ToolCallReservation::InProgress
    );
    let metadata = ToolCallMetadata {
        outcome: ToolOutcome::Succeeded,
        duration_ms: 12,
        artifact_id: None,
    };
    assert_eq!(
        repository
            .complete_tool_call(attempt.id, "call-1", &metadata)
            .await
            .unwrap(),
        ToolCallReservation::Completed(metadata.clone())
    );
    assert_eq!(
        repository
            .complete_tool_call(attempt.id, "call-1", &metadata)
            .await
            .unwrap(),
        ToolCallReservation::Completed(metadata)
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn usage_operation_replay_is_lossless_and_unique(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    let task = contract("usage", project_id, run_id, 2);
    ready(&repository, &task).await;
    let attempt = repository
        .claim_ready(2, &claim(task.id.as_str()))
        .await
        .unwrap();
    let usage = Usage {
        input_tokens: 10,
        cached_tokens: 4,
        output_tokens: 7,
        tool_calls: 3,
        latency_ms: 99,
        estimated: true,
    };
    let first = repository
        .record_usage_once(attempt.id, "model-1", &usage)
        .await
        .unwrap();
    let replay = repository
        .record_usage_once(attempt.id, "model-1", &usage)
        .await
        .unwrap();
    assert_eq!(first, replay);
    let row = sqlx::query("SELECT input_tokens,cached_tokens,output_tokens,tool_calls,latency_ms,estimated FROM model_usage WHERE agent_run_id=$1")
        .bind(attempt.id).fetch_one(&pool).await.unwrap();
    assert_eq!(
        (
            row.get::<i64, _>("input_tokens"),
            row.get::<i64, _>("cached_tokens"),
            row.get::<i64, _>("output_tokens"),
            row.get::<i64, _>("tool_calls"),
            row.get::<i64, _>("latency_ms"),
            row.get::<bool, _>("estimated")
        ),
        (10, 4, 7, 3, 99, true)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM model_usage WHERE agent_run_id=$1")
        .bind(attempt.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn artifact_callback_failure_rolls_back_file_and_metadata_has_no_path(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    let task = contract("artifact", project_id, run_id, 1);
    repository.create(&task).await.unwrap();
    let root = std::env::temp_dir().join(format!(
        "runtime-artifacts-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let store = ArtifactStore::new(&root, 1024).unwrap();
    let artifact_id = Uuid::new_v4();
    assert!(
        store
            .write(
                &artifact_id.to_string(),
                "result.txt",
                "text/plain",
                b"result",
                |_| Err::<(), _>(())
            )
            .is_err()
    );
    assert!(store.read(&artifact_id.to_string()).is_err());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM artifacts WHERE id=$1")
        .bind(artifact_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);

    repository
        .persist_artifact(&ArtifactRecord {
            id: artifact_id,
            task_id: task.id.clone(),
            kind: text("kind", "report"),
            logical_name: text("logical_name", "result.txt"),
            media_type: text("media_type", "text/plain"),
            size: 6,
            checksum: "a".repeat(64),
        })
        .await
        .unwrap();
    let row = sqlx::query("SELECT path,logical_name,media_type FROM artifacts WHERE id=$1")
        .bind(artifact_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<Option<String>, _>("path"), None);
    assert_eq!(row.get::<String, _>("logical_name"), "result.txt");
    fs::remove_dir_all(root).unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn max_attempt_atomic_rollback_and_cascade(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    let task = contract("limits", project_id, run_id, 1);
    ready(&repository, &task).await;
    let first = repository
        .claim_ready(2, &claim(task.id.as_str()))
        .await
        .unwrap();
    repository
        .reserve_tool_call(first.id, "cascade-call")
        .await
        .unwrap();
    repository
        .record_usage_once(
            first.id,
            "cascade-usage",
            &Usage {
                input_tokens: 1,
                cached_tokens: 1,
                output_tokens: 1,
                tool_calls: 1,
                latency_ms: 1,
                estimated: false,
            },
        )
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET status='READY',version=version+1 WHERE id=$1")
        .bind(task.id.as_str())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        repository.claim_ready(4, &claim(task.id.as_str())).await,
        Err(StoreError::Conflict(Conflict::Attempt))
    ));
    assert_eq!(
        repository.get(task.id.as_str()).await.unwrap().status,
        TaskStatus::Ready
    );

    sqlx::query("ALTER TABLE agent_runs ADD CONSTRAINT reject_claim_test CHECK (status <> 'assigned') NOT VALID").execute(&pool).await.unwrap();
    let other = contract("rollback", project_id, run_id, 1);
    ready(&repository, &other).await;
    assert!(matches!(
        repository.claim_ready(2, &claim(other.id.as_str())).await,
        Err(StoreError::Database(_))
    ));
    assert_eq!(
        repository.get(other.id.as_str()).await.unwrap().status,
        TaskStatus::Ready
    );
    assert_eq!(repository.events(other.id.as_str()).await.unwrap().len(), 2);

    repository.delete(task.id.as_str(), 4).await.unwrap();
    let attempts: i64 = sqlx::query_scalar("SELECT count(*) FROM agent_runs WHERE id=$1")
        .bind(first.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let reservations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tool_call_reservations WHERE agent_run_id=$1")
            .bind(first.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let usage: i64 = sqlx::query_scalar("SELECT count(*) FROM model_usage WHERE agent_run_id=$1")
        .bind(first.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((attempts, reservations, usage), (0, 0, 0));
}

#[sqlx::test(migrations = "./migrations")]
async fn assigned_start_has_exact_events_versions_and_registry_model(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    let task = contract("start", project_id, run_id, 2);
    ready(&repository, &task).await;
    let attempt = repository
        .claim_ready(2, &claim(task.id.as_str()))
        .await
        .unwrap();
    assert_eq!(attempt.status, AttemptStatus::Assigned);
    assert_eq!(repository.get(task.id.as_str()).await.unwrap().version, 3);

    let started = repository.start_claimed(attempt.id).await.unwrap();
    assert_eq!(started.status, AttemptStatus::Running);
    let stored = repository.get(task.id.as_str()).await.unwrap();
    assert_eq!((stored.status, stored.version), (TaskStatus::Running, 4));
    let events = repository.events(task.id.as_str()).await.unwrap();
    assert_eq!(events[2].to_status, Some(TaskStatus::Assigned));
    assert_eq!(events[3].to_status, Some(TaskStatus::Running));

    let rollback_task = contract("start-rollback", project_id, run_id, 2);
    ready(&repository, &rollback_task).await;
    let rollback_attempt = repository
        .claim_ready(2, &claim(rollback_task.id.as_str()))
        .await
        .unwrap();
    sqlx::query("ALTER TABLE agent_runs ADD CONSTRAINT reject_start_test CHECK (status <> 'running') NOT VALID")
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        repository.start_claimed(rollback_attempt.id).await,
        Err(StoreError::Database(_))
    ));
    assert_eq!(
        repository
            .get_attempt(rollback_attempt.id)
            .await
            .unwrap()
            .status,
        AttemptStatus::Assigned
    );
    let rollback = repository.get(rollback_task.id.as_str()).await.unwrap();
    assert_eq!(
        (rollback.status, rollback.version),
        (TaskStatus::Assigned, 3)
    );
    assert_eq!(
        repository
            .events(rollback_task.id.as_str())
            .await
            .unwrap()
            .len(),
        3
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_claim_next_has_one_winner_and_priority_order(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    let low = contract("low-priority", project_id, run_id, 2);
    let high = contract("high-priority", project_id, run_id, 2);
    ready(&repository, &low).await;
    ready(&repository, &high).await;
    sqlx::query("UPDATE tasks SET priority=10 WHERE id=$1")
        .bind(high.id.as_str())
        .execute(&pool)
        .await
        .unwrap();

    let high_attempt = repository
        .claim_next_ready(&dispatch(Uuid::new_v4()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(high_attempt.task_id, high.id);
    let high_attempt = repository.list_attempts(high.id.as_str()).await.unwrap();
    assert_eq!(high_attempt[0].model_id.as_str(), "registry-model-id");

    let left_claim = dispatch(Uuid::new_v4());
    let right_claim = dispatch(Uuid::new_v4());
    let (left, right) = tokio::join!(
        repository.claim_next_ready(&left_claim),
        repository.claim_next_ready(&right_claim)
    );
    let winners = [left.unwrap(), right.unwrap()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(winners.len(), 1);
    assert_eq!(winners[0].task_id, low.id);
}

#[sqlx::test(migrations = "./migrations")]
async fn selector_skips_task_at_max_attempts(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    let exhausted = contract("exhausted", project_id, run_id, 1);
    let eligible = contract("eligible", project_id, run_id, 1);
    ready(&repository, &exhausted).await;
    ready(&repository, &eligible).await;
    sqlx::query("UPDATE tasks SET priority=10 WHERE id=$1")
        .bind(exhausted.id.as_str())
        .execute(&pool)
        .await
        .unwrap();
    repository
        .claim_ready(2, &claim(exhausted.id.as_str()))
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET status='READY',version=version+1 WHERE id=$1")
        .bind(exhausted.id.as_str())
        .execute(&pool)
        .await
        .unwrap();

    let selected = repository
        .claim_next_ready(&dispatch(Uuid::new_v4()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(selected.task_id, eligible.id);
}

#[sqlx::test(migrations = "./migrations")]
async fn stale_assigned_and_running_recover_once(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    let assigned_task = contract("stale-assigned", project_id, run_id, 2);
    ready(&repository, &assigned_task).await;
    let assigned = repository
        .claim_ready(2, &claim(assigned_task.id.as_str()))
        .await
        .unwrap();
    sqlx::query("UPDATE agent_runs SET heartbeat_at=now()-interval '1 hour' WHERE id=$1")
        .bind(assigned.id)
        .execute(&pool)
        .await
        .unwrap();
    let cutoff = chrono_cutoff();
    let left_repository = repository.clone();
    let right_repository = repository.clone();
    let (left, right) = tokio::join!(
        left_repository.recover_stale(cutoff),
        right_repository.recover_stale(cutoff)
    );
    let recovered = [left.unwrap(), right.unwrap()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(recovered.len(), 1);
    let recovered = &recovered[0];
    assert_eq!(recovered.disposition, RecoveryDisposition::Requeued);
    let stored = repository.get(assigned_task.id.as_str()).await.unwrap();
    assert_eq!((stored.status, stored.version), (TaskStatus::Ready, 4));
    let events = repository.events(assigned_task.id.as_str()).await.unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(events[3].from_status, Some(TaskStatus::Assigned));
    assert_eq!(events[3].to_status, Some(TaskStatus::Ready));
    assert_eq!(events[3].actor, Actor::System);
    assert!(repository.recover_stale(cutoff).await.unwrap().is_none());

    let running_task = contract("stale-running", project_id, run_id, 2);
    ready(&repository, &running_task).await;
    let running = repository
        .claim_ready(2, &claim(running_task.id.as_str()))
        .await
        .unwrap();
    repository.start_claimed(running.id).await.unwrap();
    repository
        .reserve_tool_call(running.id, "completed-call")
        .await
        .unwrap();
    let metadata = ToolCallMetadata {
        outcome: ToolOutcome::Succeeded,
        duration_ms: 12,
        artifact_id: None,
    };
    repository
        .complete_tool_call(running.id, "completed-call", &metadata)
        .await
        .unwrap();
    sqlx::query("UPDATE agent_runs SET heartbeat_at=now()-interval '1 hour' WHERE id=$1")
        .bind(running.id)
        .execute(&pool)
        .await
        .unwrap();
    repository.recover_stale(cutoff).await.unwrap().unwrap();
    assert_eq!(
        repository
            .complete_tool_call(running.id, "completed-call", &metadata)
            .await
            .unwrap(),
        ToolCallReservation::Completed(metadata)
    );
    let events = repository.events(running_task.id.as_str()).await.unwrap();
    assert_eq!(events.len(), 5);
    assert_eq!(events[4].from_status, Some(TaskStatus::Running));
    assert_eq!(events[4].to_status, Some(TaskStatus::Ready));
    assert_eq!(events[4].actor, Actor::System);
    assert_eq!(
        repository
            .get(running_task.id.as_str())
            .await
            .unwrap()
            .version,
        5
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn ambiguous_reservation_is_not_replayed_and_retention_is_idempotent(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    let task = contract("ambiguous", project_id, run_id, 2);
    ready(&repository, &task).await;
    let attempt = repository
        .claim_ready(2, &claim(task.id.as_str()))
        .await
        .unwrap();
    repository.start_claimed(attempt.id).await.unwrap();
    repository
        .reserve_tool_call(attempt.id, "call")
        .await
        .unwrap();
    sqlx::query("UPDATE agent_runs SET heartbeat_at=now()-interval '1 hour' WHERE id=$1")
        .bind(attempt.id)
        .execute(&pool)
        .await
        .unwrap();
    let recovered = repository
        .recover_stale(chrono_cutoff())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.disposition, RecoveryDisposition::RecoveryRequired);
    let stored = repository.get(task.id.as_str()).await.unwrap();
    assert_eq!((stored.status, stored.version), (TaskStatus::NeedsHuman, 5));
    let events = repository.events(task.id.as_str()).await.unwrap();
    assert_eq!(events.len(), 5);
    assert_eq!(events[4].actor, Actor::System);
    assert_eq!(events[4].from_status, Some(TaskStatus::Running));
    assert_eq!(events[4].to_status, Some(TaskStatus::NeedsHuman));
    assert!(
        repository
            .claim_next_ready(&dispatch(Uuid::new_v4()))
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        repository.reserve_tool_call(attempt.id, "late-call").await,
        Err(StoreError::Conflict(Conflict::ToolCall))
    ));
    assert_eq!(
        (
            repository.get_attempt(attempt.id).await.unwrap().status,
            repository
                .get_attempt(attempt.id)
                .await
                .unwrap()
                .finished_unix_ms
                .is_some()
        ),
        (AttemptStatus::RecoveryRequired, true)
    );
    assert!(
        repository
            .recover_stale(chrono_cutoff())
            .await
            .unwrap()
            .is_none()
    );

    sqlx::query(
        "UPDATE agent_runs SET status='failed',finished_at=now(),retain_until=now() WHERE id=$1",
    )
    .bind(attempt.id)
    .execute(&pool)
    .await
    .unwrap();
    let cleanup = repository.claim_retention_due(60).await.unwrap().unwrap();
    assert_eq!(cleanup.attempt.id, attempt.id);
    assert!(
        repository
            .complete_retention_cleanup(attempt.id, cleanup.owner_token)
            .await
            .unwrap()
    );
    assert!(
        !repository
            .complete_retention_cleanup(attempt.id, cleanup.owner_token)
            .await
            .unwrap()
    );
    assert!(repository.claim_retention_due(60).await.unwrap().is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn retention_claims_are_exclusive_reclaimable_and_owner_checked(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    for name in ["retention-one", "retention-two"] {
        let task = contract(name, project_id, run_id, 2);
        ready(&repository, &task).await;
        let attempt = repository.claim_ready(2, &claim(name)).await.unwrap();
        repository
            .update_attempt(
                attempt.id,
                &AttemptUpdate {
                    status: AttemptStatus::Failed,
                    error_code: None,
                },
            )
            .await
            .unwrap();
        sqlx::query("UPDATE agent_runs SET retain_until=now() WHERE id=$1")
            .bind(attempt.id)
            .execute(&pool)
            .await
            .unwrap();
    }
    assert!(repository.claim_retention_due(0).await.is_err());
    assert!(repository.claim_retention_due(86401).await.is_err());
    let (left, right) = tokio::join!(
        repository.claim_retention_due(60),
        repository.claim_retention_due(60)
    );
    let left = left.unwrap().unwrap();
    let right = right.unwrap().unwrap();
    assert_ne!(left.attempt.id, right.attempt.id);
    assert_ne!(left.owner_token, right.owner_token);
    assert!(left.lease_until_unix_ms > chrono_cutoff());
    assert!(repository.claim_retention_due(60).await.unwrap().is_none());
    assert!(matches!(
        repository
            .complete_retention_cleanup(left.attempt.id, right.owner_token)
            .await,
        Err(StoreError::Conflict(Conflict::RetentionLease))
    ));
    sqlx::query("UPDATE agent_runs SET cleanup_lease_until=now()-interval '1 second' WHERE id=$1")
        .bind(left.attempt.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        repository
            .complete_retention_cleanup(left.attempt.id, left.owner_token)
            .await,
        Err(StoreError::Conflict(Conflict::RetentionLease))
    ));
    let (first, second) = tokio::join!(
        repository.claim_retention_due(60),
        repository.claim_retention_due(60)
    );
    let reclaimed: Vec<_> = [first.unwrap(), second.unwrap()]
        .into_iter()
        .flatten()
        .collect();
    assert_eq!(reclaimed.len(), 1);
    let reclaimed = &reclaimed[0];
    assert_eq!(reclaimed.attempt.id, left.attempt.id);
    assert_ne!(reclaimed.owner_token, left.owner_token);
    assert!(matches!(
        repository
            .complete_retention_cleanup(left.attempt.id, left.owner_token)
            .await,
        Err(StoreError::Conflict(Conflict::RetentionLease))
    ));
    let (first, second) = tokio::join!(
        repository.complete_retention_cleanup(left.attempt.id, reclaimed.owner_token),
        repository.complete_retention_cleanup(left.attempt.id, reclaimed.owner_token)
    );
    assert_ne!(first.unwrap(), second.unwrap());
    assert!(matches!(
        repository
            .complete_retention_cleanup(left.attempt.id, left.owner_token)
            .await,
        Err(StoreError::Conflict(Conflict::RetentionLease))
    ));
    assert!(
        repository
            .complete_retention_cleanup(right.attempt.id, right.owner_token)
            .await
            .unwrap()
    );
    assert!(repository.claim_retention_due(60).await.unwrap().is_none());
    let completed: i64 = sqlx::query_scalar("SELECT count(*) FROM agent_runs WHERE cleanup_state='completed' AND cleanup_completed_at IS NOT NULL AND cleanup_lease_until IS NULL")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(completed, 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn recovery_validates_each_execution_stage_and_rolls_back_atomically(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    for target in [
        TaskStatus::Running,
        TaskStatus::SelfCheck,
        TaskStatus::Review,
        TaskStatus::Verify,
        TaskStatus::Integrate,
    ] {
        for ambiguous in [false, true] {
            let name = format!("recovery-{target:?}-{ambiguous}");
            let task = contract(&name, project_id, run_id, 2);
            ready(&repository, &task).await;
            let attempt = repository.claim_ready(2, &claim(&name)).await.unwrap();
            repository.start_claimed(attempt.id).await.unwrap();
            let mut version = 4;
            if target != TaskStatus::Running {
                for (next, actor) in [
                    (TaskStatus::SelfCheck, Actor::Worker),
                    (TaskStatus::Review, Actor::Worker),
                    (TaskStatus::Verify, Actor::Reviewer),
                    (TaskStatus::Integrate, Actor::Verifier),
                ] {
                    repository
                        .transition(&name, version, next, actor)
                        .await
                        .unwrap();
                    version += 1;
                    if next == target {
                        break;
                    }
                }
            }
            if ambiguous {
                repository
                    .reserve_tool_call(attempt.id, "in-progress")
                    .await
                    .unwrap();
            }
            sqlx::query("UPDATE agent_runs SET heartbeat_at=now()-interval '1 hour' WHERE id=$1")
                .bind(attempt.id)
                .execute(&pool)
                .await
                .unwrap();
            let before_task = repository.get(&name).await.unwrap();
            let before_attempt = repository.get_attempt(attempt.id).await.unwrap();
            let before_events = repository.events(&name).await.unwrap();
            sqlx::query("ALTER TABLE events ADD CONSTRAINT reject_recovery CHECK (to_status NOT IN ('READY','NEEDS_HUMAN')) NOT VALID")
                .execute(&pool).await.unwrap();
            assert!(matches!(
                repository.recover_stale(chrono_cutoff()).await,
                Err(StoreError::Database(_))
            ));
            assert_eq!(repository.get(&name).await.unwrap(), before_task);
            assert_eq!(
                repository.get_attempt(attempt.id).await.unwrap(),
                before_attempt
            );
            assert_eq!(repository.events(&name).await.unwrap(), before_events);
            sqlx::query("ALTER TABLE events DROP CONSTRAINT reject_recovery")
                .execute(&pool)
                .await
                .unwrap();
            let (left, right) = tokio::join!(
                repository.recover_stale(chrono_cutoff()),
                repository.recover_stale(chrono_cutoff())
            );
            let recovered: Vec<_> = [left.unwrap(), right.unwrap()]
                .into_iter()
                .flatten()
                .collect();
            assert_eq!(recovered.len(), 1);
            let expected = if ambiguous {
                TaskStatus::NeedsHuman
            } else {
                TaskStatus::Ready
            };
            let stored = repository.get(&name).await.unwrap();
            assert_eq!((stored.status, stored.version), (expected, version + 1));
            let events = repository.events(&name).await.unwrap();
            assert_eq!(events.len(), before_events.len() + 1);
            let event = events.last().unwrap();
            assert_eq!(
                (event.from_status, event.to_status, event.actor),
                (Some(target), Some(expected), Actor::System)
            );
            assert!(
                repository
                    .get_attempt(attempt.id)
                    .await
                    .unwrap()
                    .finished_unix_ms
                    .is_some()
            );
            assert!(
                repository
                    .recover_stale(chrono_cutoff())
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn reservation_and_recovery_are_serialized(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    let task = contract("recovery-race", project_id, run_id, 2);
    ready(&repository, &task).await;
    let attempt = repository
        .claim_ready(2, &claim(task.id.as_str()))
        .await
        .unwrap();
    repository.start_claimed(attempt.id).await.unwrap();
    sqlx::query("UPDATE agent_runs SET heartbeat_at=now()-interval '1 hour' WHERE id=$1")
        .bind(attempt.id)
        .execute(&pool)
        .await
        .unwrap();
    let (reservation, recovery) = tokio::join!(
        repository.reserve_tool_call(attempt.id, "racing-call"),
        repository.recover_stale(chrono_cutoff())
    );
    let recovered = match recovery.unwrap() {
        Some(recovered) => recovered,
        None => repository
            .recover_stale(chrono_cutoff())
            .await
            .unwrap()
            .unwrap(),
    };
    let expected = match reservation {
        Ok(ToolCallReservation::New) => {
            assert_eq!(recovered.disposition, RecoveryDisposition::RecoveryRequired);
            TaskStatus::NeedsHuman
        }
        Err(StoreError::Conflict(Conflict::ToolCall)) => {
            assert_eq!(recovered.disposition, RecoveryDisposition::Requeued);
            TaskStatus::Ready
        }
        other => panic!("unexpected reservation result: {other:?}"),
    };
    assert_eq!(
        repository.get(task.id.as_str()).await.unwrap().status,
        expected
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn recovery_rejects_workflow_bypass_and_late_reservation(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    let task = contract("cancelled-recovery", project_id, run_id, 2);
    ready(&repository, &task).await;
    let attempt = repository
        .claim_ready(2, &claim(task.id.as_str()))
        .await
        .unwrap();
    repository
        .transition(task.id.as_str(), 3, TaskStatus::Cancelled, Actor::System)
        .await
        .unwrap();
    sqlx::query("UPDATE agent_runs SET heartbeat_at=now()-interval '1 hour' WHERE id=$1")
        .bind(attempt.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        repository.recover_stale(chrono_cutoff()).await,
        Err(StoreError::InvalidTransition(_))
    ));
    assert!(
        repository
            .get_attempt(attempt.id)
            .await
            .unwrap()
            .finished_unix_ms
            .is_none()
    );
    assert_eq!(
        repository.get(task.id.as_str()).await.unwrap().status,
        TaskStatus::Cancelled
    );
    repository
        .update_attempt(
            attempt.id,
            &AttemptUpdate {
                status: AttemptStatus::Failed,
                error_code: None,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        repository.reserve_tool_call(attempt.id, "late-call").await,
        Err(StoreError::Conflict(Conflict::ToolCall))
    ));
}

fn chrono_cutoff() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
