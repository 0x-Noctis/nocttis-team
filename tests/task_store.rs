use ai_team::{
    domain::{
        state_machine::Actor,
        task::{
            AllowedPath, MAX_SAFE_INTEGER, MaxAttempts, NonEmptyString, PositiveLimit,
            TaskContract, TaskLimits, TaskStatus,
        },
    },
    store::{
        event::{AgentAttempt, Usage},
        task::{Conflict, StoreError, TaskRepository},
    },
};
use sqlx::{PgPool, Row};
use uuid::Uuid;

fn text(field: &'static str, value: impl Into<String>) -> NonEmptyString {
    NonEmptyString::parse(field, value).unwrap()
}

async fn ownership(pool: &PgPool) -> (Uuid, Uuid) {
    let project_id = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();
    let run_id = Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap();
    sqlx::query("INSERT INTO projects (id,name,repository_path) VALUES ($1,'project',$2)")
        .bind(project_id)
        .bind(format!("/repo/{project_id}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'objective','running',1)")
        .bind(run_id).bind(project_id).execute(pool).await.unwrap();
    (project_id, run_id)
}

fn contract(id: &str, project_id: Uuid, run_id: Uuid) -> TaskContract {
    TaskContract {
        id: text("id", id),
        project_id: text("project_id", project_id.to_string()),
        project_run_id: text("project_run_id", run_id.to_string()),
        title: text("title", "Implement task runtime"),
        role: text("role", "control_plane_engineer"),
        objective: text("objective", "Persist task safely"),
        depends_on: Vec::new(),
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

#[sqlx::test(migrations = "./migrations")]
async fn migration_uses_only_canonical_tables(pool: PgPool) {
    let forbidden: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.tables WHERE table_schema='public'
         AND table_name IN ('runtime_tasks','task_events','agent_attempts','task_usage')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(forbidden, 0);
    for table in ["tasks", "events", "agent_runs", "model_usage"] {
        let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(format!("public.{table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(exists, "missing {table}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn legacy_failed_final_is_migrated_to_failed(pool: PgPool) {
    let schema = format!("upgrade_{}", Uuid::new_v4().simple());
    let mut connection = pool.acquire().await.unwrap();
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query(&format!("SET search_path TO {schema}"))
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0001_initial.sql"))
        .execute(&mut *connection)
        .await
        .unwrap();
    let project_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    let task_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name,repository_path) VALUES ($1,'legacy','/legacy')")
        .bind(project_id)
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'legacy','done',1)")
        .bind(run_id)
        .bind(project_id)
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO tasks (id,project_run_id,role,title,objective,status,input_token_limit,output_token_limit) VALUES ($1,$2,'worker','legacy','legacy','FAILED_FINAL',1,1)")
        .bind(task_id)
        .bind(run_id)
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0004_task_runtime.sql"))
        .execute(&mut *connection)
        .await
        .unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id=$1")
        .bind(task_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(status, "FAILED");
}

#[sqlx::test(migrations = "./migrations")]
async fn project_identifiers_require_canonical_uuid_text(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool);
    let canonical = contract("canonical", project_id, run_id);
    let stored = repository.create(&canonical).await.unwrap();
    assert_eq!(stored.contract.project_id.as_str(), project_id.to_string());
    assert_eq!(stored.contract.project_run_id.as_str(), run_id.to_string());

    for (field, uuid) in [("project_id", project_id), ("project_run_id", run_id)] {
        for (index, value) in [
            uuid.to_string().to_uppercase(),
            uuid.simple().to_string(),
            uuid.braced().to_string(),
            uuid.urn().to_string(),
        ]
        .into_iter()
        .enumerate()
        {
            let mut invalid = contract(&format!("invalid-{field}-{index}"), project_id, run_id);
            match field {
                "project_id" => invalid.project_id = text("project_id", value),
                _ => invalid.project_run_id = text("project_run_id", value),
            }
            assert!(matches!(
                repository.create(&invalid).await,
                Err(StoreError::InvalidId(error_field)) if error_field == field
            ));
        }
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn full_contract_roundtrip_uses_project_ownership_and_dependencies(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool);
    let dependency = contract("ARCH-001", project_id, run_id);
    repository.create(&dependency).await.unwrap();
    let mut original = contract("TASK-001", project_id, run_id);
    original.depends_on = vec![text("depends_on", "ARCH-001")];
    assert_eq!(
        repository.create(&original).await.unwrap().contract,
        original
    );
    assert_eq!(repository.get("TASK-001").await.unwrap().contract, original);
    repository.delete("TASK-001", 0).await.unwrap();
    assert!(matches!(
        repository.get("TASK-001").await,
        Err(StoreError::NotFound)
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn delete_requires_callers_expected_version(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool);
    let mut value = contract("delete-version", project_id, run_id);
    repository.create(&value).await.unwrap();
    value.title = text("title", "updated");
    repository.update(&value, 0).await.unwrap();

    assert!(matches!(
        repository.delete("delete-version", 0).await,
        Err(StoreError::Conflict(Conflict::StaleVersion))
    ));
    repository.delete("delete-version", 1).await.unwrap();
    assert!(matches!(
        repository.delete("delete-version", 1).await,
        Err(StoreError::NotFound)
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_delete_has_one_winner(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool);
    repository
        .create(&contract("delete-race", project_id, run_id))
        .await
        .unwrap();

    let (left, right) = tokio::join!(
        repository.delete("delete-race", 0),
        repository.delete("delete-race", 0)
    );
    assert!(matches!(
        (&left, &right),
        (Ok(()), Err(StoreError::NotFound)) | (Err(StoreError::NotFound), Ok(()))
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn update_requires_callers_expected_version(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool);
    let mut value = contract("update", project_id, run_id);
    repository.create(&value).await.unwrap();
    value.title = text("title", "new title");
    assert_eq!(repository.update(&value, 0).await.unwrap().version, 1);
    assert!(matches!(
        repository.update(&value, 0).await,
        Err(StoreError::Conflict(Conflict::StaleVersion))
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn transition_and_event_are_atomic(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    repository
        .create(&contract("atomic", project_id, run_id))
        .await
        .unwrap();
    sqlx::query("ALTER TABLE events ADD CONSTRAINT reject_transition_test CHECK (event_type <> 'status_transition')")
        .execute(&pool).await.unwrap();
    assert!(matches!(
        repository
            .transition("atomic", 0, TaskStatus::Planned, Actor::System)
            .await,
        Err(StoreError::Database(_))
    ));
    let stored = repository.get("atomic").await.unwrap();
    assert_eq!((stored.status, stored.version), (TaskStatus::Draft, 0));
    assert!(repository.events("atomic").await.unwrap().is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn successful_transition_appends_constrained_event(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool);
    repository
        .create(&contract("transition", project_id, run_id))
        .await
        .unwrap();
    let stored = repository
        .transition("transition", 0, TaskStatus::Planned, Actor::System)
        .await
        .unwrap();
    assert_eq!((stored.status, stored.version), (TaskStatus::Planned, 1));
    let events = repository.events("transition").await.unwrap();
    assert_eq!(
        (events[0].from_status, events[0].to_status),
        (Some(TaskStatus::Draft), Some(TaskStatus::Planned))
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_transition_has_one_winner(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool);
    repository
        .create(&contract("race", project_id, run_id))
        .await
        .unwrap();
    let left_repo = repository.clone();
    let right_repo = repository.clone();
    let (left, right) = tokio::join!(
        left_repo.transition("race", 0, TaskStatus::Planned, Actor::System),
        right_repo.transition("race", 0, TaskStatus::Planned, Actor::System)
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert!(matches!(
        left.err().or(right.err()).unwrap(),
        StoreError::Conflict(Conflict::StaleVersion)
    ));
    assert_eq!(repository.events("race").await.unwrap().len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn usage_roundtrips_estimated_true_and_false(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool);
    repository
        .create(&contract("usage", project_id, run_id))
        .await
        .unwrap();
    let attempt = AgentAttempt {
        id: Uuid::new_v4(),
        task_id: text("task_id", "usage"),
        role: text("role", "worker"),
        attempt: MAX_SAFE_INTEGER,
        provider_id: text("provider_id", "provider"),
        model_id: text("model_id", "model"),
        status: text("status", "running"),
    };
    repository.create_attempt(&attempt).await.unwrap();
    for estimated in [true, false] {
        let usage = Usage {
            input_tokens: MAX_SAFE_INTEGER,
            cached_tokens: 2,
            output_tokens: 3,
            tool_calls: 4,
            latency_ms: 5,
            estimated,
        };
        let id = repository.record_usage(attempt.id, &usage).await.unwrap();
        assert_eq!(repository.usage(id).await.unwrap(), usage);
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn project_cascade_reaches_task_event_attempt_and_usage(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    repository
        .create(&contract("cascade", project_id, run_id))
        .await
        .unwrap();
    repository
        .transition("cascade", 0, TaskStatus::Planned, Actor::System)
        .await
        .unwrap();
    let attempt = AgentAttempt {
        id: Uuid::new_v4(),
        task_id: text("task_id", "cascade"),
        role: text("role", "worker"),
        attempt: 1,
        provider_id: text("provider_id", "provider"),
        model_id: text("model_id", "model"),
        status: text("status", "done"),
    };
    repository.create_attempt(&attempt).await.unwrap();
    repository
        .record_usage(
            attempt.id,
            &Usage {
                input_tokens: 1,
                cached_tokens: 0,
                output_tokens: 1,
                tool_calls: 0,
                latency_ms: 1,
                estimated: false,
            },
        )
        .await
        .unwrap();
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    for table in [
        "projects",
        "project_runs",
        "tasks",
        "events",
        "agent_runs",
        "model_usage",
    ] {
        let count: i64 = sqlx::query(&format!("SELECT count(*) AS count FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("count");
        assert_eq!(count, 0, "{table}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn safe_integer_boundaries_are_enforced(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool.clone());
    repository
        .create(&contract("safe", project_id, run_id))
        .await
        .unwrap();
    let error = sqlx::query("UPDATE tasks SET max_tool_calls=$2 WHERE id=$1")
        .bind("safe")
        .bind(MAX_SAFE_INTEGER + 1)
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(error.as_database_error().is_some());
    let attempt = AgentAttempt {
        id: Uuid::new_v4(),
        task_id: text("task_id", "safe"),
        role: text("role", "worker"),
        attempt: MAX_SAFE_INTEGER + 1,
        provider_id: text("provider_id", "provider"),
        model_id: text("model_id", "model"),
        status: text("status", "running"),
    };
    assert!(matches!(
        repository.create_attempt(&attempt).await,
        Err(StoreError::InvalidNumeric(_))
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn pagination_is_deterministic(pool: PgPool) {
    let (project_id, run_id) = ownership(&pool).await;
    let repository = TaskRepository::new(pool);
    for id in ["task-c", "task-a", "task-b"] {
        repository
            .create(&contract(id, project_id, run_id))
            .await
            .unwrap();
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
