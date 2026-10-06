// M4-001: claim task atomik untuk banyak scheduler (src/store/scheduler.rs).
use std::{collections::HashSet, sync::Arc};

use ai_team::{
    domain::{
        project::{
            ApprovalDecision, PlanApproval, PlanStatus, Project, ProjectRun, ProposedPlan,
            RepositoryPath, RunStatus,
        },
        task::{AllowedPath, MaxAttempts, NonEmptyString, PositiveLimit, TaskContract, TaskLimits},
    },
    store::{
        event::RecoveryDisposition,
        project::ProjectRepository,
        scheduler::{ClaimRequest, ClaimedTask, SchedulerStore},
        task::{Conflict, StoreError, TaskRepository},
    },
};
use sqlx::PgPool;
use uuid::Uuid;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
// Tiap task memesan 100 + 50 token.
const NEEDED: i64 = 150;

fn text(value: impl Into<String>) -> NonEmptyString {
    NonEmptyString::parse("value", value).unwrap()
}

fn limit(value: i64) -> PositiveLimit {
    PositiveLimit::new("limit", value).unwrap()
}

fn contract(id: &str, project: Uuid, run: Uuid, depends_on: &[&str]) -> TaskContract {
    TaskContract {
        id: text(id),
        project_id: text(project.to_string()),
        project_run_id: text(run.to_string()),
        title: text("task"),
        role: text("worker"),
        objective: text("ship"),
        depends_on: depends_on.iter().map(|id| text(*id)).collect(),
        allowed_paths: vec![AllowedPath::parse(format!("src/{id}/**")).unwrap()],
        context_refs: vec![],
        acceptance_criteria: vec![text("passes")],
        verification_commands: vec![text("cargo test")],
        limits: TaskLimits {
            max_input_tokens: limit(100),
            max_output_tokens: limit(50),
            max_tool_calls: limit(5),
            max_attempts: MaxAttempts::new(2).unwrap(),
            timeout_seconds: limit(60),
        },
    }
}

/// Project + run RUNNING dengan budget tertentu; mengembalikan (project, run).
async fn run_with_budget(pool: &PgPool, budget: i64) -> (Uuid, Uuid) {
    let (project, run) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name,repository_path) VALUES ($1,'project',$2)")
        .bind(project)
        .bind(format!("/repo/{project}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'obj','RUNNING',$3)")
        .bind(run)
        .bind(project)
        .bind(budget)
        .execute(pool)
        .await
        .unwrap();
    (project, run)
}

/// Task manual (tanpa plan) berstatus READY dengan priority tertentu.
async fn ready_task(
    pool: &PgPool,
    id: &str,
    project: Uuid,
    run: Uuid,
    priority: i32,
    deps: &[&str],
) {
    TaskRepository::new(pool.clone())
        .create(&contract(id, project, run, deps))
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET status='READY',priority=$2 WHERE id=$1")
        .bind(id)
        .bind(priority)
        .execute(pool)
        .await
        .unwrap();
}

async fn claim(store: &SchedulerStore, owner: Uuid) -> Option<ClaimedTask> {
    store
        .claim_next(
            &ClaimRequest {
                owner,
                provider_id: "provider",
                model_id: "model",
                retention_seconds: 60,
            },
            |_| Some(COMMIT.to_owned()),
        )
        .await
        .unwrap()
}

fn id_of(claimed: &ClaimedTask) -> String {
    claimed.attempt.task_id.as_str().to_owned()
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_claims_never_share_a_task(pool: PgPool) {
    let (project, run) = run_with_budget(&pool, 1_000_000).await;
    for index in 0..6 {
        ready_task(&pool, &format!("t{index}"), project, run, 0, &[]).await;
    }
    let store = Arc::new(SchedulerStore::new(pool.clone()));
    let handles: Vec<_> = (0..12)
        .map(|_| {
            let store = store.clone();
            tokio::spawn(async move { claim(&store, Uuid::new_v4()).await })
        })
        .collect();
    let mut claimed = Vec::new();
    for handle in handles {
        claimed.extend(handle.await.unwrap());
    }
    let ids: HashSet<_> = claimed.iter().map(id_of).collect();
    assert_eq!(
        (claimed.len(), ids.len()),
        (6, 6),
        "setiap task diklaim tepat sekali"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tasks WHERE status='ASSIGNED'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        6
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_runs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        6
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM events WHERE event_type='status_transition' AND to_status='ASSIGNED'")
            .fetch_one(&pool).await.unwrap(),
        6
    );
    assert!(claim(&store, Uuid::new_v4()).await.is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn claim_order_is_priority_then_age_then_id(pool: PgPool) {
    let (project, run) = run_with_budget(&pool, 1_000_000).await;
    ready_task(&pool, "low-old", project, run, 0, &[]).await;
    ready_task(&pool, "mid", project, run, 5, &[]).await;
    ready_task(&pool, "high", project, run, 9, &[]).await;
    ready_task(&pool, "mid-newer", project, run, 5, &[]).await;
    let store = SchedulerStore::new(pool);
    let mut order = Vec::new();
    while let Some(claimed) = claim(&store, Uuid::new_v4()).await {
        order.push(id_of(&claimed));
    }
    assert_eq!(order, ["high", "mid", "mid-newer", "low-old"]);
}

#[sqlx::test(migrations = "./migrations")]
async fn ineligible_candidates_do_not_block_runnable_ones(pool: PgPool) {
    // Budget hanya cukup untuk satu task manual (150) dari 200.
    let (project, run) = run_with_budget(&pool, 200).await;
    ready_task(&pool, "dep", project, run, 0, &[]).await;
    sqlx::query("UPDATE tasks SET status='RUNNING' WHERE id='dep'")
        .execute(&pool)
        .await
        .unwrap();
    // Prioritas tertinggi tetapi dependency belum DONE -> harus dilewati.
    ready_task(&pool, "blocked", project, run, 9, &["dep"]).await;
    ready_task(&pool, "runnable", project, run, 1, &[]).await;
    let store = SchedulerStore::new(pool.clone());
    assert_eq!(
        id_of(&claim(&store, Uuid::new_v4()).await.unwrap()),
        "runnable"
    );
    assert!(claim(&store, Uuid::new_v4()).await.is_none());

    // Dependency selesai tetapi budget run sudah terpakai -> tidak diklaim dan reservasi tidak berubah.
    sqlx::query("UPDATE tasks SET status='DONE' WHERE id='dep'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(claim(&store, Uuid::new_v4()).await.is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT reserved_tokens FROM project_runs WHERE id=$1")
            .bind(run)
            .fetch_one(&pool)
            .await
            .unwrap(),
        NEEDED
    );

    // Run di-pause -> tidak ada claim sama sekali.
    let (project2, run2) = run_with_budget(&pool, 1_000).await;
    ready_task(&pool, "paused-task", project2, run2, 0, &[]).await;
    sqlx::query("UPDATE project_runs SET status='PAUSED' WHERE id=$1")
        .bind(run2)
        .execute(&pool)
        .await
        .unwrap();
    assert!(claim(&store, Uuid::new_v4()).await.is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn manual_reservations_never_exceed_budget_under_race(pool: PgPool) {
    let (project, run) = run_with_budget(&pool, NEEDED * 3).await;
    for index in 0..8 {
        ready_task(&pool, &format!("m{index}"), project, run, 0, &[]).await;
    }
    let store = Arc::new(SchedulerStore::new(pool.clone()));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let store = store.clone();
            tokio::spawn(async move { claim(&store, Uuid::new_v4()).await })
        })
        .collect();
    let mut won = 0;
    for handle in handles {
        won += usize::from(handle.await.unwrap().is_some());
    }
    assert_eq!(won, 3);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT reserved_tokens FROM project_runs WHERE id=$1")
            .bind(run)
            .fetch_one(&pool)
            .await
            .unwrap(),
        NEEDED * 3
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn approved_plan_tasks_respect_dependencies_and_reservation(pool: PgPool) {
    let (project, run) = (Uuid::new_v4(), Uuid::new_v4());
    let projects = ProjectRepository::new(pool.clone());
    projects
        .create_project(&Project {
            id: text(project.to_string()),
            name: text("p"),
            repository_path: RepositoryPath::parse(".").unwrap(),
        })
        .await
        .unwrap();
    projects
        .create_run(&ProjectRun {
            id: text(run.to_string()),
            project_id: text(project.to_string()),
            objective: text("ship"),
            acceptance_criteria: vec![text("ok")],
            token_budget: limit(1_000),
            status: RunStatus::Planning,
        })
        .await
        .unwrap();
    let plan = ProposedPlan {
        id: text("plan-1"),
        project_run_id: text(run.to_string()),
        version: limit(1),
        tasks: vec![
            contract("root", project, run, &[]),
            contract("leaf", project, run, &["root"]),
        ],
        risk_flags: vec![],
        status: PlanStatus::Proposed,
    };
    projects.propose(&plan).await.unwrap();
    projects
        .decide(&PlanApproval {
            plan_id: text("plan-1"),
            actor_id: text("human"),
            decision: ApprovalDecision::Approved,
            reason: None,
        })
        .await
        .unwrap();

    let store = SchedulerStore::new(pool.clone());
    let root = claim(&store, Uuid::new_v4())
        .await
        .expect("root dapat diklaim");
    assert_eq!(id_of(&root), "root");
    // Task plan tidak menaikkan reservasi saat claim (sudah dipesan saat approval).
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT reserved_tokens FROM project_runs WHERE id=$1")
            .bind(run)
            .fetch_one(&pool)
            .await
            .unwrap(),
        NEEDED * 2
    );
    assert!(
        claim(&store, Uuid::new_v4()).await.is_none(),
        "leaf menunggu root DONE"
    );
    sqlx::query("UPDATE tasks SET status='DONE' WHERE id='root'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(id_of(&claim(&store, Uuid::new_v4()).await.unwrap()), "leaf");
}

#[sqlx::test(migrations = "./migrations")]
async fn heartbeat_ownership_stale_detection_and_recovery(pool: PgPool) {
    let (project, run) = run_with_budget(&pool, 1_000_000).await;
    ready_task(&pool, "only", project, run, 0, &[]).await;
    let store = SchedulerStore::new(pool.clone());
    let (alice, bob) = (Uuid::new_v4(), Uuid::new_v4());
    let first = claim(&store, alice).await.unwrap();

    store.heartbeat(first.attempt.id, alice).await.unwrap();
    assert!(matches!(
        store.heartbeat(first.attempt.id, bob).await,
        Err(StoreError::Conflict(Conflict::Claim))
    ));
    assert!(
        store.stale_claims(30, 10).await.unwrap().is_empty(),
        "claim sehat tidak stale"
    );

    // Alice berhenti heartbeat -> stale; dua scheduler memulihkan bersamaan, hanya satu yang menang.
    sqlx::query("UPDATE agent_runs SET heartbeat_at=now()-interval '1 hour' WHERE id=$1")
        .bind(first.attempt.id)
        .execute(&pool)
        .await
        .unwrap();
    let stale = store.stale_claims(30, 10).await.unwrap();
    assert_eq!((stale.len(), stale[0].owner), (1, Some(alice)));
    let (left, right) = tokio::join!(
        store.recover_abandoned(30, 10),
        store.recover_abandoned(30, 10)
    );
    let recovered: Vec<_> = left.unwrap().into_iter().chain(right.unwrap()).collect();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].1, RecoveryDisposition::Requeued);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status::text FROM tasks WHERE id='only'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "READY"
    );
    // Pemegang lama kehilangan claim dan tidak bisa lagi menulis.
    assert!(matches!(
        store.heartbeat(first.attempt.id, alice).await,
        Err(StoreError::Conflict(Conflict::Claim))
    ));

    // Scheduler lain mengambilnya sebagai attempt 2; setelah ditinggalkan lagi, attempt habis (max 2).
    let second = claim(&store, bob).await.unwrap();
    assert_eq!(second.attempt.attempt, 2);
    sqlx::query("UPDATE agent_runs SET heartbeat_at=now()-interval '1 hour' WHERE id=$1")
        .bind(second.attempt.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(store.recover_abandoned(30, 10).await.unwrap().len(), 1);
    assert!(
        claim(&store, Uuid::new_v4()).await.is_none(),
        "attempt habis, task tidak diklaim lagi"
    );
}

// Spesifikasi SKIP LOCKED yang deterministik: kandidat teratas sedang dikunci transaksi lain ->
// claim harus mengambil task berikutnya, bukan menunggu (atau gagal) sampai lock dilepas.
#[sqlx::test(migrations = "./migrations")]
async fn locked_top_candidate_is_skipped_not_waited_on(pool: PgPool) {
    let (project, run) = run_with_budget(&pool, 1_000_000).await;
    ready_task(&pool, "top", project, run, 9, &[]).await;
    ready_task(&pool, "next", project, run, 0, &[]).await;
    let store = SchedulerStore::new(pool.clone());

    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM tasks WHERE id='top' FOR UPDATE")
        .execute(&mut *holder)
        .await
        .unwrap();
    let claimed = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        claim(&store, Uuid::new_v4()),
    )
    .await
    .expect("claim tidak boleh menunggu baris yang dikunci")
    .expect("task berikutnya harus diklaim");
    assert_eq!(id_of(&claimed), "next");
    holder.rollback().await.unwrap();
    assert_eq!(id_of(&claim(&store, Uuid::new_v4()).await.unwrap()), "top");
}
