mod domain {
    pub use ai_team::domain::*;
}
#[path = "../src/store/project.rs"]
mod project_store;

use ai_team::domain::{
    project::{
        ApprovalDecision, PlanApproval, PlanStatus, Project, ProjectRun, ProposedPlan,
        RepositoryPath, RunStatus,
    },
    task::{AllowedPath, MaxAttempts, NonEmptyString, PositiveLimit, TaskContract, TaskLimits},
};
use project_store::{ProjectRepository, ProjectStoreError};
use sqlx::{PgPool, Row};
use uuid::Uuid;

fn text(value: impl Into<String>) -> NonEmptyString {
    NonEmptyString::parse("value", value).unwrap()
}
fn limit(value: i64) -> PositiveLimit {
    PositiveLimit::new("limit", value).unwrap()
}

async fn setup(pool: PgPool) -> (ProjectRepository, Uuid, Uuid) {
    let project_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    let store = ProjectRepository::new(pool);
    store
        .create_project(&Project {
            id: text(project_id.to_string()),
            name: text("project"),
            repository_path: RepositoryPath::parse(".").unwrap(),
        })
        .await
        .unwrap();
    store
        .create_run(&ProjectRun {
            id: text(run_id.to_string()),
            project_id: text(project_id.to_string()),
            objective: text("ship"),
            acceptance_criteria: vec![text("checks pass")],
            token_budget: limit(100),
            status: RunStatus::Planning,
        })
        .await
        .unwrap();
    (store, project_id, run_id)
}

fn plan(project: Uuid, run: Uuid, version: i64, ids: &[&str]) -> ProposedPlan {
    let tasks = ids
        .iter()
        .enumerate()
        .map(|(index, id)| TaskContract {
            id: text(*id),
            project_id: text(project.to_string()),
            project_run_id: text(run.to_string()),
            title: text("task"),
            role: text("worker"),
            objective: text("ship"),
            depends_on: if index == 0 {
                vec![]
            } else {
                vec![text(ids[0])]
            },
            allowed_paths: vec![AllowedPath::parse("src/**").unwrap()],
            context_refs: vec![],
            acceptance_criteria: vec![text("passes")],
            verification_commands: vec![text("cargo test")],
            limits: TaskLimits {
                max_input_tokens: limit(20),
                max_output_tokens: limit(10),
                max_tool_calls: limit(1),
                max_attempts: MaxAttempts::new(1).unwrap(),
                timeout_seconds: limit(1),
            },
        })
        .collect();
    ProposedPlan {
        id: text(format!("plan-{version}")),
        project_run_id: text(run.to_string()),
        version: limit(version),
        tasks,
        risk_flags: vec![text("review")],
        status: PlanStatus::Proposed,
    }
}

fn decision(plan_id: &str, choice: ApprovalDecision) -> PlanApproval {
    PlanApproval {
        plan_id: text(plan_id),
        actor_id: text("human"),
        decision: choice,
        reason: (choice == ApprovalDecision::Rejected).then(|| text("needs changes")),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn versions_are_immutable_and_reuse_canonical_tasks(pool: PgPool) {
    let (store, project, run) = setup(pool.clone()).await;
    assert_eq!(
        store
            .get_project(&project.to_string())
            .await
            .unwrap()
            .name
            .as_str(),
        "project"
    );
    assert_eq!(
        store
            .get_run(&run.to_string())
            .await
            .unwrap()
            .acceptance_criteria[0]
            .as_str(),
        "checks pass"
    );
    let first = plan(project, run, 1, &["a", "b"]);
    store.propose(&first).await.unwrap();
    assert_eq!(
        store.get_run(&run.to_string()).await.unwrap().status,
        RunStatus::AwaitingApproval
    );
    store
        .decide(&decision("plan-1", ApprovalDecision::Approved))
        .await
        .unwrap();
    assert_eq!(store.get_plan("plan-1").await.unwrap().tasks, first.tasks);
    let row = sqlx::query("SELECT reserved_tokens FROM project_runs WHERE id=$1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<i64, _>("reserved_tokens"), 60);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM task_dependencies WHERE task_id='b' AND dependency_id='a'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    assert!(
        sqlx::query("UPDATE plans SET tasks='[]' WHERE id='plan-1'")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE tasks SET title='changed' WHERE id='a'")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM task_dependencies WHERE task_id='b'")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM tasks WHERE id='a'")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(matches!(
        store
            .decide(&decision("plan-1", ApprovalDecision::Approved))
            .await,
        Err(ProjectStoreError::Conflict)
    ));
    let second = plan(project, run, 2, &["c"]);
    store.propose(&second).await.unwrap();
    store
        .decide(&decision("plan-2", ApprovalDecision::Approved))
        .await
        .unwrap();
    let row = sqlx::query("SELECT reserved_tokens FROM project_runs WHERE id=$1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<i64, _>("reserved_tokens"), 90);
    assert_eq!(store.get_plan("plan-1").await.unwrap().tasks, first.tasks);
    assert_eq!(store.get_plan("plan-2").await.unwrap().tasks, second.tasks);
}

#[sqlx::test(migrations = "./migrations")]
async fn failed_materialization_rolls_back_tasks_dependencies_and_budget(pool: PgPool) {
    let (store, project, run) = setup(pool.clone()).await;
    store
        .propose(&plan(project, run, 1, &["a", "b"]))
        .await
        .unwrap();
    sqlx::query("ALTER TABLE tasks ADD CONSTRAINT reject_plan_task CHECK (id <> 'b') NOT VALID")
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store
            .decide(&decision("plan-1", ApprovalDecision::Approved))
            .await,
        Err(ProjectStoreError::Database(_))
    ));
    let (tasks, dependencies, reserved): (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM tasks WHERE project_run_id=$1), (SELECT count(*) FROM task_dependencies), reserved_tokens FROM project_runs WHERE id=$1"
    ).bind(run).fetch_one(&pool).await.unwrap();
    assert_eq!((tasks, dependencies, reserved), (0, 0, 0));
    assert_eq!(
        store.get_plan("plan-1").await.unwrap().status,
        PlanStatus::Proposed
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn manual_dependency_delete_removes_rows(pool: PgPool) {
    let (_, _, run) = setup(pool.clone()).await;
    for id in ["manual-a", "manual-b"] {
        sqlx::query("INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,max_attempts) VALUES ($1,$2,'worker','manual','manual','DRAFT','[]','[]','[]',10,10,2)")
            .bind(id)
            .bind(run)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query(
        "INSERT INTO task_dependencies (task_id,dependency_id) VALUES ('manual-b','manual-a')",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        sqlx::query("DELETE FROM task_dependencies WHERE task_id='manual-b'")
            .execute(&pool)
            .await
            .unwrap()
            .rows_affected(),
        1
    );
    assert_eq!(
        sqlx::query("DELETE FROM tasks WHERE id='manual-a'")
            .execute(&pool)
            .await
            .unwrap()
            .rows_affected(),
        1
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn approval_race_and_budget_rollback(pool: PgPool) {
    let (store, project, run) = setup(pool.clone()).await;
    store
        .propose(&plan(project, run, 1, &["a", "b", "c", "d"]))
        .await
        .unwrap();
    let approval = decision("plan-1", ApprovalDecision::Approved);
    assert!(matches!(
        store.decide(&approval).await,
        Err(ProjectStoreError::Conflict)
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE project_run_id=$1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        store.get_plan("plan-1").await.unwrap().status,
        PlanStatus::Proposed
    );
    store
        .decide(&decision("plan-1", ApprovalDecision::Rejected))
        .await
        .unwrap();
    store.propose(&plan(project, run, 2, &["e"])).await.unwrap();
    let approval = decision("plan-2", ApprovalDecision::Approved);
    let (left, right) = tokio::join!(store.decide(&approval), store.decide(&approval));
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert_eq!(
        store.get_plan("plan-2").await.unwrap().status,
        PlanStatus::Approved
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE project_run_id=$1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}
