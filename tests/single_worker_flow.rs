use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};

use ai_team::{
    domain::task::TaskStatus,
    orchestrator::{Orchestrator, OrchestratorConfig, StartRequest, claim_task},
    runner::git::GitWorktreeManager,
    store::{
        artifact::ArtifactStore,
        provider::ProviderRepository,
        task::{Conflict, StoreError, TaskRepository},
    },
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[sqlx::test(migrations = "./migrations")]
async fn production_orchestrator_conflict_preserves_target(pool: PgPool) {
    production_flow(pool, true).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn production_orchestrator_completes_full_flow(pool: PgPool) {
    production_flow(pool, false).await;
}

async fn production_flow(pool: PgPool, conflict: bool) {
    let fixture = FlowFixture::new();
    let patch = "diff --git a/tracked.txt b/tracked.txt\n--- a/tracked.txt\n+++ b/tracked.txt\n@@ -1 +1 @@\n-base\n+changed\n";
    let responses = vec![
        json!({
            "choices": [{
                "message": {"content": null, "tool_calls": [{
                    "id": "patch-1", "type": "function", "function": {
                        "name": "apply_patch", "arguments": json!({"patch": patch}).to_string()
                    }
                }]},
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
        }),
        json!({
            "choices": [{
                "message": {"content": json!({"summary": "patched", "status": "self_check"}).to_string()},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5}
        }),
        json!({
            "choices": [{
                "message": {"content": json!({"decision": "approved"}).to_string()},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 4, "completion_tokens": 1, "total_tokens": 5}
        }),
    ];
    let (base_url, server) = fake_provider(responses).await;
    unsafe {
        std::env::set_var("FLOW_API_KEY", "test-only");
        std::env::set_var("NOCTIS_RUNNER_IMAGE", "dbisynergy-frontend:dev");
    }
    let project_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    let task_id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO projects (id,name,repository_path) VALUES ($1,'flow',$2)")
        .bind(project_id)
        .bind(fixture.repository.to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'flow','running',1000)").bind(run_id).bind(project_id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO providers (id,base_url,api_key_env,request_timeout_seconds) VALUES ('flow-e2e-provider',$1,'FLOW_API_KEY',5)").bind(&base_url).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO models (id,provider_id,remote_name,class,context_window,max_output_tokens,claimed_capabilities,verified_capabilities) VALUES ('flow-e2e-model','flow-e2e-provider','mock','coding',1000,1000,'{\"chat\":true,\"streaming\":false,\"tools\":true,\"parallel_tools\":false}','{\"chat\":\"unknown\",\"streaming\":\"unknown\",\"tools\":\"unknown\",\"parallel_tools\":\"unknown\"}')").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,max_attempts,max_tool_calls,timeout_seconds) VALUES ($1,$2,'worker','flow','flow','READY','[\"tracked.txt\"]','[\"works\"]','[\"test -f tracked.txt\"]',1000,1000,2,10,60)").bind(&task_id).bind(run_id).execute(&pool).await.unwrap();
    let tasks = TaskRepository::new(pool.clone());
    let providers = ProviderRepository::new(pool.clone());
    let attempt_id = Uuid::new_v4();
    claim_task(
        &tasks,
        &providers,
        StartRequest {
            task_id: &task_id,
            expected_version: 0,
            model_id: "flow-e2e-model",
            retention_seconds: 60,
            attempt_id,
        },
    )
    .await
    .unwrap();
    if conflict {
        let manager = GitWorktreeManager::new(&fixture.repository, &fixture.worktrees).unwrap();
        let target_id = format!("{task_id}-integration");
        let target = manager
            .create(&target_id, &format!("integration-{task_id}"), &fixture.base)
            .unwrap();
        fs::write(target.path().join("tracked.txt"), "target\n").unwrap();
        git(target.path(), &["add", "tracked.txt"]);
        git(target.path(), &["commit", "-m", "target change"]);
    }
    let artifacts = ArtifactStore::new(&fixture.artifacts, 1024 * 1024).unwrap();
    let mut orchestrator = Orchestrator::new(
        tasks.clone(),
        providers,
        artifacts,
        OrchestratorConfig {
            worktree_root: fixture.worktrees.clone(),
            stale_after_seconds: 60,
            retention_lease_seconds: 60,
        },
    );
    assert!(orchestrator.dispatch_once().await.unwrap());
    server.await.unwrap();

    let stored = tasks.get(&task_id).await.unwrap();
    assert_eq!(
        stored.status,
        if conflict {
            TaskStatus::Conflict
        } else {
            TaskStatus::Done
        }
    );
    assert_eq!(
        tasks.get_attempt(attempt_id).await.unwrap().status,
        if conflict {
            ai_team::store::event::AttemptStatus::Failed
        } else {
            ai_team::store::event::AttemptStatus::Completed
        }
    );
    let transitions: Vec<_> = tasks
        .events(&task_id)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|event| event.to_status)
        .collect();
    assert_eq!(
        transitions,
        vec![
            TaskStatus::Assigned,
            TaskStatus::Running,
            TaskStatus::SelfCheck,
            TaskStatus::Review,
            TaskStatus::Verify,
            TaskStatus::Integrate,
            if conflict {
                TaskStatus::Conflict
            } else {
                TaskStatus::Done
            }
        ]
    );
    let target = fixture.worktrees.join(format!("{task_id}-integration"));
    assert_eq!(
        fs::read_to_string(target.join("tracked.txt")).unwrap(),
        if conflict { "target\n" } else { "changed\n" }
    );
    assert_eq!(
        git_output(&fixture.repository, &["rev-parse", "main"]),
        fixture.base
    );
    assert!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM model_usage WHERE agent_run_id=$1")
            .bind(attempt_id)
            .fetch_one(&pool)
            .await
            .unwrap()
            >= 2
    );
    assert!(fs::read_dir(&fixture.artifacts).unwrap().next().is_some());
}

#[sqlx::test(migrations = "./migrations")]
async fn start_is_atomic_and_ambiguous_recovery_never_replays(pool: PgPool) {
    let root = std::env::temp_dir().join(format!(
        "noctis-single-worker-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let repository = root.join("repository");
    fs::create_dir_all(repository.join(".git/refs/heads")).unwrap();
    fs::write(repository.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(
        repository.join(".git/refs/heads/main"),
        "0123456789012345678901234567890123456789\n",
    )
    .unwrap();
    let project_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    let task_id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO projects (id,name,repository_path) VALUES ($1,'flow',$2)")
        .bind(project_id)
        .bind(repository.to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'flow','running',1000)")
        .bind(run_id).bind(project_id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO providers (id,base_url,api_key_env,request_timeout_seconds) VALUES ('flow-provider','http://127.0.0.1:1','FLOW_API_KEY',1)")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO models (id,provider_id,remote_name,class,context_window,max_output_tokens,claimed_capabilities,verified_capabilities) VALUES ('flow-model','flow-provider','mock','coding',1000,1000,'{\"chat\":true,\"streaming\":false,\"tools\":true,\"parallel_tools\":false}','{\"chat\":\"unknown\",\"streaming\":\"unknown\",\"tools\":\"unknown\",\"parallel_tools\":\"unknown\"}')")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,max_attempts) VALUES ($1,$2,'worker','flow','flow','READY','[\"src/**\"]','[\"works\"]','[\"cargo test\"]',1000,1000,2)")
        .bind(&task_id).bind(run_id).execute(&pool).await.unwrap();

    let tasks = TaskRepository::new(pool.clone());
    let providers = ProviderRepository::new(pool.clone());
    let first_id = Uuid::new_v4();
    let first = claim_task(
        &tasks,
        &providers,
        StartRequest {
            task_id: &task_id,
            expected_version: 0,
            model_id: "flow-model",
            retention_seconds: 60,
            attempt_id: first_id,
        },
    )
    .await
    .unwrap();
    assert_eq!(first.status, TaskStatus::Assigned);
    let second = claim_task(
        &tasks,
        &providers,
        StartRequest {
            task_id: &task_id,
            expected_version: 0,
            model_id: "flow-model",
            retention_seconds: 60,
            attempt_id: Uuid::new_v4(),
        },
    )
    .await;
    assert!(matches!(second, Err(StoreError::Conflict(Conflict::Claim))));
    assert_eq!(tasks.list_attempts(&task_id).await.unwrap().len(), 1);

    tasks.start_claimed(first_id).await.unwrap();
    tasks
        .reserve_tool_call(first_id, "call-1", "apply_patch")
        .await
        .unwrap();
    sqlx::query("UPDATE agent_runs SET heartbeat_at=to_timestamp(0) WHERE id=$1")
        .bind(first_id)
        .execute(&pool)
        .await
        .unwrap();
    let artifacts = ArtifactStore::new(root.join("artifacts"), 1024).unwrap();
    let orchestrator = Orchestrator::new(
        tasks.clone(),
        providers,
        artifacts,
        OrchestratorConfig {
            worktree_root: root.join("worktrees"),
            stale_after_seconds: 1,
            retention_lease_seconds: 30,
        },
    );
    assert_eq!(orchestrator.startup_recovery().await.unwrap(), 1);
    assert_eq!(
        tasks.get(&task_id).await.unwrap().status,
        TaskStatus::NeedsHuman
    );
    assert_eq!(
        tasks
            .get_attempt(first_id)
            .await
            .unwrap()
            .error_code
            .as_deref(),
        Some("recovery.tool_in_progress")
    );
    let _ = fs::remove_dir_all(root);
}

struct FlowFixture {
    root: std::path::PathBuf,
    repository: std::path::PathBuf,
    worktrees: std::path::PathBuf,
    artifacts: std::path::PathBuf,
    base: String,
}

impl FlowFixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "noctis-flow-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let repository = root.join("repository");
        let worktrees = root.join("worktrees");
        let artifacts = root.join("artifacts");
        fs::create_dir_all(&repository).unwrap();
        git(&repository, &["init", "--initial-branch=main"]);
        git(&repository, &["config", "user.name", "Noctis Test"]);
        git(
            &repository,
            &["config", "user.email", "noctis@example.invalid"],
        );
        fs::write(repository.join("tracked.txt"), "base\n").unwrap();
        git(&repository, &["add", "tracked.txt"]);
        git(&repository, &["commit", "-m", "initial"]);
        let base = git_output(&repository, &["rev-parse", "HEAD"]);
        Self {
            root,
            repository,
            worktrees,
            artifacts,
            base,
        }
    }
}

impl Drop for FlowFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn git(path: &std::path::Path, arguments: &[&str]) {
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(arguments)
            .status()
            .unwrap()
            .success()
    );
}

fn git_output(path: &std::path::Path, arguments: &[&str]) -> String {
    String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(arguments)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned()
}

async fn fake_provider(responses: Vec<serde_json::Value>) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        for response in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 65536];
            let _ = stream.read(&mut request).await.unwrap();
            let body = response.to_string();
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
        }
    });
    (format!("http://{address}/v1"), handle)
}
