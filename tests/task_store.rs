pub use ai_team::domain;

#[path = "../src/store/event.rs"]
mod event;
#[path = "../src/store/task.rs"]
mod task;

use domain::{
    state_machine::Actor,
    task::{
        AllowedPath, MAX_SAFE_INTEGER, MaxAttempts, NonEmptyString, PositiveLimit, TaskContract,
        TaskLimits, TaskStatus,
    },
};
use event::{AgentAttempt, Usage};
use sqlx::{PgPool, Row};
use task::{Conflict, StoreError, TaskRepository};
use uuid::Uuid;

fn text(field: &'static str, value: &str) -> NonEmptyString {
    NonEmptyString::parse(field, value).unwrap()
}

fn contract(id: &str) -> TaskContract {
    TaskContract {
        id: text("id", id),
        project_id: text("project_id", "project-a"),
        title: text("title", "Implement task runtime"),
        role: text("role", "control_plane_engineer"),
        objective: text("objective", "Persist task safely"),
        depends_on: vec![text("depends_on", "ARCH-001")],
        allowed_paths: vec![AllowedPath::parse("src/tasks/**").unwrap()],
        context_refs: vec![text("context_refs", "artifact://decision")],
        acceptance_criteria: vec![text("acceptance_criteria", "Roundtrip works")],
        verification_commands: vec![text("verification_commands", "cargo test task_store")],
        limits: TaskLimits {
            max_input_tokens: PositiveLimit::new("max_input_tokens", MAX_SAFE_INTEGER).unwrap(),
            max_output_tokens: PositiveLimit::new("max_output_tokens", 8_000).unwrap(),
            max_tool_calls: PositiveLimit::new("max_tool_calls", 40).unwrap(),
            max_attempts: MaxAttempts::new(10).unwrap(),
            timeout_seconds: PositiveLimit::new("timeout_seconds", 1_200).unwrap(),
        },
    }
}

async fn advance(repository: &TaskRepository, id: &str, from_version: i64, to: TaskStatus) {
    repository
        .transition(id, from_version, to, Actor::System)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn migration_from_empty_database_creates_runtime_schema(pool: PgPool) {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name FROM information_schema.tables
         WHERE table_schema='public' AND table_name IN
         ('runtime_tasks','task_events','agent_attempts','task_usage') ORDER BY table_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        tables,
        [
            "agent_attempts",
            "runtime_tasks",
            "task_events",
            "task_usage"
        ]
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn task_crud_roundtrips_complete_contract_and_limits(pool: PgPool) {
    let repository = TaskRepository::new(pool);
    let original = contract("task-a");
    let created = repository.create(&original).await.unwrap();
    assert_eq!(created.contract, original);
    assert_eq!(created.status, TaskStatus::Draft);
    assert_eq!(repository.get("task-a").await.unwrap(), created);

    let mut updated = original.clone();
    updated.title = text("title", "Updated title");
    updated.limits.max_output_tokens =
        PositiveLimit::new("max_output_tokens", MAX_SAFE_INTEGER).unwrap();
    let stored = repository.update(&updated).await.unwrap();
    assert_eq!(stored.contract, updated);
    assert_eq!(stored.version, 1);

    repository.delete("task-a").await.unwrap();
    assert!(matches!(
        repository.get("task-a").await,
        Err(StoreError::NotFound)
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn reconstructs_every_status(pool: PgPool) {
    let repository = TaskRepository::new(pool.clone());
    let statuses = [
        TaskStatus::Draft,
        TaskStatus::Planned,
        TaskStatus::Ready,
        TaskStatus::Assigned,
        TaskStatus::Running,
        TaskStatus::SelfCheck,
        TaskStatus::Review,
        TaskStatus::ChangesRequested,
        TaskStatus::Verify,
        TaskStatus::Failed,
        TaskStatus::Integrate,
        TaskStatus::Conflict,
        TaskStatus::NeedsHuman,
        TaskStatus::Done,
        TaskStatus::Cancelled,
    ];
    for (index, status) in statuses.into_iter().enumerate() {
        let id = format!("status-{index}");
        repository.create(&contract(&id)).await.unwrap();
        let status_text = serde_json::to_value(status)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        sqlx::query("UPDATE runtime_tasks SET status=$2 WHERE id=$1")
            .bind(&id)
            .bind(status_text)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(repository.get(&id).await.unwrap().status, status);
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn transition_updates_status_and_appends_event(pool: PgPool) {
    let repository = TaskRepository::new(pool);
    repository.create(&contract("transition")).await.unwrap();
    let transitioned = repository
        .transition("transition", 0, TaskStatus::Planned, Actor::System)
        .await
        .unwrap();
    assert_eq!(transitioned.status, TaskStatus::Planned);
    assert_eq!(transitioned.version, 1);
    let events = repository.events("transition").await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].from_status, Some(TaskStatus::Draft));
    assert_eq!(events[0].to_status, Some(TaskStatus::Planned));
    assert_eq!(events[0].actor, Actor::System);
}

#[sqlx::test(migrations = "./migrations")]
async fn invalid_and_stale_transitions_are_rejected(pool: PgPool) {
    let repository = TaskRepository::new(pool);
    repository.create(&contract("invalid")).await.unwrap();
    assert!(matches!(
        repository
            .transition("invalid", 0, TaskStatus::Done, Actor::Worker)
            .await,
        Err(StoreError::InvalidTransition(_))
    ));
    advance(&repository, "invalid", 0, TaskStatus::Planned).await;
    assert!(matches!(
        repository
            .transition("invalid", 0, TaskStatus::Ready, Actor::System)
            .await,
        Err(StoreError::Conflict(Conflict::StaleTransition))
    ));
    assert_eq!(repository.events("invalid").await.unwrap().len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_transition_allows_only_one_winner(pool: PgPool) {
    let repository = TaskRepository::new(pool);
    repository.create(&contract("race")).await.unwrap();
    let first = repository.clone();
    let second = repository.clone();
    let (left, right) = tokio::join!(
        first.transition("race", 0, TaskStatus::Planned, Actor::System),
        second.transition("race", 0, TaskStatus::Planned, Actor::System)
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert!(matches!(
        left.as_ref().err().or(right.as_ref().err()).unwrap(),
        StoreError::Conflict(Conflict::StaleTransition)
    ));
    assert_eq!(repository.events("race").await.unwrap().len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn event_failure_rolls_back_status_update(pool: PgPool) {
    let repository = TaskRepository::new(pool.clone());
    repository.create(&contract("rollback")).await.unwrap();
    sqlx::query(
        "ALTER TABLE task_events ADD CONSTRAINT reject_transition_test
         CHECK (event_type <> 'status_transition')",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        repository
            .transition("rollback", 0, TaskStatus::Planned, Actor::System)
            .await,
        Err(StoreError::Database(_))
    ));
    let stored = repository.get("rollback").await.unwrap();
    assert_eq!(stored.status, TaskStatus::Draft);
    assert_eq!(stored.version, 0);
    assert!(repository.events("rollback").await.unwrap().is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn pagination_is_stable_and_deterministic(pool: PgPool) {
    let repository = TaskRepository::new(pool);
    for id in ["task-c", "task-a", "task-b"] {
        repository.create(&contract(id)).await.unwrap();
    }
    let first = repository.list(None, 2).await.unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.contract.id.as_str())
            .collect::<Vec<_>>(),
        ["task-a", "task-b"]
    );
    let second = repository
        .list(first.next_cursor.as_deref(), 2)
        .await
        .unwrap();
    assert_eq!(second.items[0].contract.id.as_str(), "task-c");
    assert!(second.next_cursor.is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn attempts_usage_and_cascade_obey_safe_integer_contract(pool: PgPool) {
    let repository = TaskRepository::new(pool.clone());
    repository.create(&contract("owned")).await.unwrap();
    let attempt = AgentAttempt {
        id: Uuid::new_v4(),
        task_id: text("task_id", "owned"),
        attempt: MAX_SAFE_INTEGER,
        provider_id: text("provider_id", "provider"),
        model_id: text("model_id", "model"),
        status: text("status", "running"),
    };
    repository.create_attempt(&attempt).await.unwrap();
    let usage = Usage {
        input_tokens: MAX_SAFE_INTEGER,
        cached_tokens: 0,
        output_tokens: 1,
        tool_calls: 2,
        latency_ms: MAX_SAFE_INTEGER,
    };
    repository.record_usage(attempt.id, &usage).await.unwrap();

    let invalid_attempt = AgentAttempt {
        attempt: MAX_SAFE_INTEGER + 1,
        id: Uuid::new_v4(),
        ..attempt.clone()
    };
    assert!(matches!(
        repository.create_attempt(&invalid_attempt).await,
        Err(StoreError::InvalidNumeric(_))
    ));
    let invalid_usage = Usage {
        input_tokens: MAX_SAFE_INTEGER + 1,
        ..usage
    };
    assert!(matches!(
        repository.record_usage(attempt.id, &invalid_usage).await,
        Err(StoreError::InvalidNumeric(_))
    ));

    repository
        .transition("owned", 0, TaskStatus::Planned, Actor::System)
        .await
        .unwrap();
    repository.delete("owned").await.unwrap();
    for table in ["task_events", "agent_attempts", "task_usage"] {
        let count: i64 = sqlx::query(&format!("SELECT count(*) AS count FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("count");
        assert_eq!(count, 0, "{table}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn database_rejects_values_above_safe_integer(pool: PgPool) {
    let repository = TaskRepository::new(pool.clone());
    repository.create(&contract("unsafe-db")).await.unwrap();
    let error = sqlx::query("UPDATE runtime_tasks SET max_input_tokens=$2 WHERE id=$1")
        .bind("unsafe-db")
        .bind(MAX_SAFE_INTEGER + 1)
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(error.as_database_error().is_some());
}
