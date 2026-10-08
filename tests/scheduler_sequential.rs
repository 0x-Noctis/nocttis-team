use ai_team::{
    orchestrator::{Orchestrator, OrchestratorConfig, SequentialScheduler},
    store::{artifact::ArtifactStore, provider::ProviderRepository, task::TaskRepository},
};
use sqlx::{PgPool, Row};
use uuid::Uuid;

async fn setup(pool: &PgPool, budget: i64) -> (Uuid, SequentialScheduler) {
    let project = Uuid::new_v4();
    let run = Uuid::new_v4();
    let repo = std::env::current_dir().unwrap().canonicalize().unwrap();
    sqlx::query("INSERT INTO projects (id,name,repository_path) VALUES ($1,'scheduler',$2)")
        .bind(project)
        .bind(repo.to_str().unwrap())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'scheduler','RUNNING',$3)")
        .bind(run).bind(project).bind(budget).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO providers (id,base_url,api_key_env,request_timeout_seconds) VALUES ('scheduler-provider','http://127.0.0.1:1','SCHEDULER_TEST_KEY',1)")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO models (id,provider_id,remote_name,class,context_window,max_output_tokens) VALUES ('scheduler-model','scheduler-provider','mock','coding',1000,1000)")
        .execute(pool).await.unwrap();
    (
        run,
        SequentialScheduler::new(pool.clone(), "scheduler-model".into(), 60),
    )
}

async fn task(pool: &PgPool, run: Uuid, id: &str, priority: i32) {
    sqlx::query("INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,max_attempts,priority) VALUES ($1,$2,'worker','task','task','READY','[\"src/**\"]','[\"works\"]','[\"cargo test\"]',10,10,2,$3)")
        .bind(id).bind(run).bind(priority).execute(pool).await.unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn dependency_order_priority_and_blocked_task(pool: PgPool) {
    let (run, scheduler) = setup(&pool, 100).await;
    task(&pool, run, "root", 0).await;
    task(&pool, run, "child", 20).await;
    task(&pool, run, "last", 30).await;
    task(&pool, run, "independent", 10).await;
    sqlx::query("INSERT INTO task_dependencies (task_id,dependency_id) VALUES ('child','root'),('last','child')")
        .execute(&pool).await.unwrap();
    let first = scheduler.claim_one().await.unwrap().unwrap();
    assert_eq!(first.task_id.as_str(), "independent");
    assert!(scheduler.claim_one().await.unwrap().is_none());
    sqlx::query("UPDATE tasks SET status='DONE' WHERE id=$1")
        .bind(first.task_id.as_str())
        .execute(&pool)
        .await
        .unwrap();
    let second = scheduler.claim_one().await.unwrap().unwrap();
    assert_eq!(second.task_id.as_str(), "root");
    sqlx::query("UPDATE tasks SET status='DONE' WHERE id='root'")
        .execute(&pool)
        .await
        .unwrap();
    let third = scheduler.claim_one().await.unwrap().unwrap();
    assert_eq!(third.task_id.as_str(), "child");
    sqlx::query("UPDATE tasks SET status='DONE' WHERE id='child'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        scheduler
            .claim_one()
            .await
            .unwrap()
            .unwrap()
            .task_id
            .as_str(),
        "last"
    );
    assert!(scheduler.claim_one().await.unwrap().is_none());
    let reserved: i64 = sqlx::query_scalar("SELECT reserved_tokens FROM project_runs WHERE id=$1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(reserved, 80);
}

#[sqlx::test(migrations = "./migrations")]
async fn pause_cancel_and_budget_stop_before_claim(pool: PgPool) {
    let (run, scheduler) = setup(&pool, 19).await;
    task(&pool, run, "waiting", 0).await;
    sqlx::query("UPDATE project_runs SET status='PAUSED' WHERE id=$1")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    assert!(scheduler.claim_one().await.unwrap().is_none());
    sqlx::query("UPDATE project_runs SET status='RUNNING' WHERE id=$1")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    assert!(scheduler.claim_one().await.unwrap().is_none());
    sqlx::query("UPDATE project_runs SET token_budget=20 WHERE id=$1")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        scheduler
            .claim_one()
            .await
            .unwrap()
            .unwrap()
            .task_id
            .as_str(),
        "waiting"
    );
    task(&pool, run, "later", 0).await;
    sqlx::query("UPDATE tasks SET status='DONE' WHERE id='waiting'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE project_runs SET status='CANCELLED' WHERE id=$1")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    assert!(scheduler.claim_one().await.unwrap().is_none());
    let (reserved, count): (i64, i64) = {
        let row = sqlx::query("SELECT reserved_tokens,(SELECT count(*) FROM agent_runs) AS count FROM project_runs WHERE id=$1")
            .bind(run).fetch_one(&pool).await.unwrap();
        (row.get("reserved_tokens"), row.get("count"))
    };
    assert_eq!((reserved, count), (20, 1));
}

#[sqlx::test(migrations = "./migrations")]
async fn approved_plan_uses_existing_reservation(pool: PgPool) {
    let (run, scheduler) = setup(&pool, 20).await;
    sqlx::query(
        "INSERT INTO plans (id,project_run_id,version,tasks) VALUES ('approved',$1,1,'[{}]')",
    )
    .bind(run)
    .execute(&pool)
    .await
    .unwrap();
    task(&pool, run, "planned", 0).await;
    sqlx::query("UPDATE tasks SET plan_id='approved' WHERE id='planned'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(scheduler.claim_one().await.unwrap().is_none());
    sqlx::query("UPDATE plans SET status='APPROVED' WHERE id='approved'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE project_runs SET reserved_tokens=20 WHERE id=$1")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        scheduler
            .claim_one()
            .await
            .unwrap()
            .unwrap()
            .task_id
            .as_str(),
        "planned"
    );
    let reserved: i64 = sqlx::query_scalar("SELECT reserved_tokens FROM project_runs WHERE id=$1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(reserved, 20);
}

#[sqlx::test(migrations = "./migrations")]
async fn planned_task_requires_approved_reservation(pool: PgPool) {
    let (run, scheduler) = setup(&pool, 40).await;
    sqlx::query(
        "INSERT INTO plans (id,project_run_id,version,tasks) VALUES ('wrong-run',$1,1,'[{}]')",
    )
    .bind(run)
    .execute(&pool)
    .await
    .unwrap();
    task(&pool, run, "planned", 5).await;
    sqlx::query("UPDATE tasks SET plan_id='wrong-run' WHERE id='planned'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(scheduler.claim_one().await.unwrap().is_none());
    sqlx::query("UPDATE project_runs SET reserved_tokens=10 WHERE id=$1")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE plans SET status='APPROVED' WHERE id='wrong-run'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(scheduler.claim_one().await.unwrap().is_none());
    sqlx::query("UPDATE project_runs SET reserved_tokens=20 WHERE id=$1")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        scheduler
            .claim_one()
            .await
            .unwrap()
            .unwrap()
            .task_id
            .as_str(),
        "planned"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn budget_blocked_priority_does_not_starve_other_run(pool: PgPool) {
    let (_, scheduler) = setup(&pool, 19).await;
    let run = Uuid::new_v4();
    let project: Uuid = sqlx::query_scalar("SELECT project_id FROM project_runs LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'another','RUNNING',20)")
        .bind(run).bind(project).execute(&pool).await.unwrap();
    task(&pool, run, "affordable", 0).await;
    let first_run: Uuid = sqlx::query_scalar("SELECT id FROM project_runs WHERE id<>$1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    task(&pool, first_run, "expensive", 10).await;
    assert_eq!(
        scheduler
            .claim_one()
            .await
            .unwrap()
            .unwrap()
            .task_id
            .as_str(),
        "affordable"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn sequential_dispatch_runs_m2_setup_lifecycle_before_next_task(pool: PgPool) {
    let (run, scheduler) = setup(&pool, 40).await;
    task(&pool, run, "first", 0).await;
    task(&pool, run, "second", 10).await;
    sqlx::query("INSERT INTO task_dependencies (task_id,dependency_id) VALUES ('second','first')")
        .execute(&pool)
        .await
        .unwrap();
    let root = std::env::temp_dir().join(format!("scheduler-test-{}", Uuid::new_v4()));
    let orchestrator = Orchestrator::new(
        TaskRepository::new(pool.clone()),
        ProviderRepository::new(pool.clone()),
        ArtifactStore::new(root.join("artifacts"), 1024).unwrap(),
        OrchestratorConfig {
            worktree_root: root.join("worktrees"),
            stale_after_seconds: 60,
            retention_lease_seconds: 60,
        },
    );
    assert!(
        orchestrator
            .dispatch_sequential_once(&scheduler)
            .await
            .unwrap()
    );
    let tasks = TaskRepository::new(pool.clone());
    let first = tasks.get("first").await.unwrap();
    assert_eq!(first.status, ai_team::domain::task::TaskStatus::Ready);
    assert_eq!(
        tasks.get("second").await.unwrap().status,
        ai_team::domain::task::TaskStatus::Ready
    );
    assert_eq!(tasks.list_attempts("first").await.unwrap().len(), 1);
    assert_eq!(
        tasks
            .events("first")
            .await
            .unwrap()
            .iter()
            .filter_map(|event| event.to_status)
            .collect::<Vec<_>>(),
        vec![
            ai_team::domain::task::TaskStatus::Assigned,
            ai_team::domain::task::TaskStatus::Ready
        ]
    );
    assert!(
        orchestrator
            .dispatch_sequential_once(&scheduler)
            .await
            .unwrap()
    );
    assert_eq!(tasks.list_attempts("first").await.unwrap().len(), 2);
    assert!(scheduler.claim_one().await.unwrap().is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_claim_is_unique_and_budget_atomic(pool: PgPool) {
    let (run, first) = setup(&pool, 20).await;
    task(&pool, run, "only", 0).await;
    let second = SequentialScheduler::new(pool.clone(), "scheduler-model".into(), 60);
    let (a, b) = tokio::join!(first.claim_one(), second.claim_one());
    let claimed: Vec<_> = [a.unwrap(), b.unwrap()].into_iter().flatten().collect();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].task_id.as_str(), "only");
    assert_eq!(
        TaskRepository::new(pool.clone())
            .list_attempts("only")
            .await
            .unwrap()
            .len(),
        1
    );
    let reserved: i64 = sqlx::query_scalar("SELECT reserved_tokens FROM project_runs WHERE id=$1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(reserved, 20);
}

#[sqlx::test(migrations = "./migrations")]
async fn multi_task_pipeline_follows_dependencies(pool: PgPool) {
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let root = std::env::temp_dir().join(format!("scheduler-flow-{}", Uuid::new_v4()));
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    for args in [
        vec!["init", "--initial-branch=main"],
        vec!["config", "user.name", "Noctis Test"],
        vec!["config", "user.email", "noctis@example.invalid"],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(repo.join("tracked.txt"), "base\n").unwrap();
    for args in [vec!["add", "tracked.txt"], vec!["commit", "-m", "initial"]] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let mut responses = Vec::new();
    // "second" bergantung pada "first": worker-nya mulai dari hasil terintegrasi, jadi patch-nya relatif ke "first".
    for (i, (before, value)) in [("base", "first"), ("first", "second")]
        .into_iter()
        .enumerate()
    {
        let patch = format!(
            "diff --git a/tracked.txt b/tracked.txt\n--- a/tracked.txt\n+++ b/tracked.txt\n@@ -1 +1 @@\n-{before}\n+{value}\n"
        );
        responses.push(json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":format!("patch-{i}"),"type":"function","function":{"name":"apply_patch","arguments":json!({"patch":patch}).to_string()}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}}));
        responses.push(json!({"choices":[{"message":{"content":json!({"summary":"patched","status":"self_check"}).to_string()},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}));
        responses.push(json!({"choices":[{"message":{"content":json!({"decision":"approved"}).to_string()},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":1,"total_tokens":5}}));
    }
    let server = tokio::spawn(async move {
        for response in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 65536];
            assert!(stream.read(&mut request).await.unwrap() > 0);
            let body = response.to_string();
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
    });
    unsafe {
        // Server tiruan di loopback: izinkan hostnya secara eksplisit (anti-SSRF berlaku di setiap panggilan).
        std::env::set_var("NOCTIS_PROVIDER_HOST_ALLOWLIST", "127.0.0.1,localhost");
        std::env::set_var("SCHEDULER_TEST_KEY", "test-only");
        std::env::set_var("NOCTIS_RUNNER_IMAGE", "dbisynergy-frontend:dev");
    }
    let project = Uuid::new_v4();
    let run = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name,repository_path) VALUES ($1,'flow',$2)")
        .bind(project)
        .bind(repo.to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'flow','RUNNING',4000)")
        .bind(run).bind(project).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO providers (id,base_url,api_key_env,request_timeout_seconds) VALUES ('flow-provider',$1,'SCHEDULER_TEST_KEY',5)")
        .bind(url).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO models (id,provider_id,remote_name,class,context_window,max_output_tokens,claimed_capabilities,verified_capabilities) VALUES ('flow-model','flow-provider','mock','coding',1000,1000,'{\"chat\":true,\"streaming\":false,\"tools\":true,\"parallel_tools\":false}','{\"chat\":\"unknown\",\"streaming\":\"unknown\",\"tools\":\"supported\",\"parallel_tools\":\"unknown\"}')")
        .execute(&pool).await.unwrap();
    for (id, priority) in [("first", 0), ("second", 10)] {
        sqlx::query("INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,max_attempts,max_tool_calls,timeout_seconds,priority) VALUES ($1,$2,'worker','flow','flow','READY','[\"tracked.txt\"]','[\"works\"]','[\"test -f tracked.txt\"]',1000,1000,2,10,60,$3)")
            .bind(id).bind(run).bind(priority).execute(&pool).await.unwrap();
    }
    sqlx::query("INSERT INTO task_dependencies (task_id,dependency_id) VALUES ('second','first')")
        .execute(&pool)
        .await
        .unwrap();
    let scheduler = SequentialScheduler::new(pool.clone(), "flow-model".into(), 60);
    let tasks = TaskRepository::new(pool.clone());
    let orchestrator = Orchestrator::new(
        tasks.clone(),
        ProviderRepository::new(pool.clone()),
        ArtifactStore::new(root.join("artifacts"), 1024 * 1024).unwrap(),
        OrchestratorConfig {
            worktree_root: root.join("worktrees"),
            stale_after_seconds: 60,
            retention_lease_seconds: 60,
        },
    );
    tokio::time::timeout(std::time::Duration::from_secs(120), async {
        assert!(
            orchestrator
                .dispatch_sequential_once(&scheduler)
                .await
                .unwrap()
        );
        assert_eq!(
            tasks.get("first").await.unwrap().status,
            ai_team::domain::task::TaskStatus::Done
        );
        assert_eq!(tasks.list_attempts("second").await.unwrap().len(), 0);
        assert!(
            orchestrator
                .dispatch_sequential_once(&scheduler)
                .await
                .unwrap()
        );
    })
    .await
    .expect("sequential flow timed out");
    server.await.unwrap();
    assert_eq!(
        tasks.get("second").await.unwrap().status,
        ai_team::domain::task::TaskStatus::Done
    );
    assert!(
        tasks
            .events("first")
            .await
            .unwrap()
            .iter()
            .any(|event| event.to_status == Some(ai_team::domain::task::TaskStatus::Verify))
    );
    assert!(
        tasks
            .events("second")
            .await
            .unwrap()
            .iter()
            .any(|event| event.to_status == Some(ai_team::domain::task::TaskStatus::Verify))
    );
    assert!(
        !orchestrator
            .dispatch_sequential_once(&scheduler)
            .await
            .unwrap()
    );
    std::fs::remove_dir_all(root).unwrap();
}

// Regression: model default belum terdaftar (mis. provider baru dibuat lewat UI setelah server start)
// dulu membuat claim_one gagal tiap polling dengan "model_id must be a UUID", dan task READY terblokir
// tanpa petunjuk. Sekarang scheduler menunggu tanpa error dan tanpa efek samping.
#[sqlx::test(migrations = "./migrations")]
async fn missing_model_waits_without_error_or_side_effect(pool: PgPool) {
    let (run, _) = setup(&pool, 40).await;
    let scheduler = SequentialScheduler::new(pool.clone(), "not-registered-yet".into(), 60);

    // Idle: tanpa task READY pun tidak boleh error.
    assert!(scheduler.claim_one().await.unwrap().is_none());

    task(&pool, run, "waiting", 0).await;
    assert!(scheduler.claim_one().await.unwrap().is_none());
    let row = sqlx::query("SELECT status FROM tasks WHERE id='waiting'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<&str, _>("status"), "READY");
    let attempts: i64 = sqlx::query_scalar("SELECT count(*) FROM agent_runs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(attempts, 0);
    // Reservasi budget harus ikut rollback; kalau bocor, retry berikutnya kehabisan budget.
    let reserved: i64 = sqlx::query_scalar("SELECT reserved_tokens FROM project_runs WHERE id=$1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(reserved, 0);

    // Begitu model terdaftar, task yang sama langsung diklaim.
    sqlx::query("INSERT INTO models (id,provider_id,remote_name,class,context_window,max_output_tokens) VALUES ('not-registered-yet','scheduler-provider','mock-late','coding',1000,1000)")
        .execute(&pool)
        .await
        .unwrap();
    let attempt = scheduler.claim_one().await.unwrap().unwrap();
    assert_eq!(attempt.task_id.as_str(), "waiting");
}

#[sqlx::test(migrations = "./migrations")]
async fn missing_model_does_not_fail_sequential_dispatch(pool: PgPool) {
    let (run, _) = setup(&pool, 40).await;
    task(&pool, run, "waiting", 0).await;
    let scheduler = SequentialScheduler::new(pool.clone(), "not-registered-yet".into(), 60);
    let root = std::env::temp_dir().join(format!("scheduler-test-{}", Uuid::new_v4()));
    let orchestrator = Orchestrator::new(
        TaskRepository::new(pool.clone()),
        ProviderRepository::new(pool.clone()),
        ArtifactStore::new(root.join("artifacts"), 1024).unwrap(),
        OrchestratorConfig {
            worktree_root: root.join("worktrees"),
            stale_after_seconds: 60,
            retention_lease_seconds: 60,
        },
    );
    assert!(
        !orchestrator
            .dispatch_sequential_once(&scheduler)
            .await
            .unwrap()
    );
    std::fs::remove_dir_all(root).ok();
}
