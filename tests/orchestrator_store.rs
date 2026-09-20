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
            ArtifactRecord, AttemptStatus, AttemptUpdate, ClaimAttempt, RuntimeAttempt,
            ToolCallMetadata, ToolCallReservation, ToolOutcome, Usage,
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
        TaskStatus::Running
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
    assert_eq!(events.last().unwrap().to_status, Some(TaskStatus::Running));
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

    sqlx::query("ALTER TABLE agent_runs ADD CONSTRAINT reject_claim_test CHECK (status <> 'running') NOT VALID").execute(&pool).await.unwrap();
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
