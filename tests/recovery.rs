// M4-008: recovery startup (src/recovery.rs) — crash/restart pada setiap state penting, dengan repository
// Git dan PostgreSQL sungguhan.
use std::{fs, path::PathBuf, process::Command};

use ai_team::{
    domain::{
        budget::Purpose,
        task::{AllowedPath, MaxAttempts, NonEmptyString, PositiveLimit, TaskContract, TaskLimits},
    },
    recovery::{RECOVERY_LOCK, RecoveryConfig, recover},
    runner::git::GitWorktreeManager,
    store::{
        budget::{BudgetStore, ReserveRequest},
        lease::LeaseStore,
        scheduler::{ClaimRequest, SchedulerStore},
        task::TaskRepository,
    },
};
use sqlx::PgPool;
use uuid::Uuid;

fn text(value: impl Into<String>) -> NonEmptyString {
    NonEmptyString::parse("value", value).unwrap()
}

fn limit(value: i64) -> PositiveLimit {
    PositiveLimit::new("limit", value).unwrap()
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

struct Env {
    pool: PgPool,
    root: PathBuf,
    manager: GitWorktreeManager,
    head: String,
    project: Uuid,
    run: Uuid,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

async fn env(pool: PgPool) -> Env {
    let root = std::env::temp_dir().join(format!("noctis-recovery-{}", Uuid::new_v4()));
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&root, &["init", "-q", "-b", "main", repo.to_str().unwrap()]);
    git(&repo, &["config", "user.name", "Fixture"]);
    git(&repo, &["config", "user.email", "fixture@example.invalid"]);
    fs::write(repo.join("a.txt"), "base\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let manager = GitWorktreeManager::new(&repo, root.join("worktrees")).unwrap();
    let (project, run) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name,repository_path) VALUES ($1,'project',$2)")
        .bind(project)
        .bind(manager.repository_root().to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'obj','RUNNING',1000000)")
        .bind(run)
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();
    Env {
        pool,
        root,
        manager,
        head,
        project,
        run,
    }
}

impl Env {
    fn config(&self, stale_after_seconds: i64) -> RecoveryConfig {
        RecoveryConfig {
            worktree_root: self.manager.worktree_root().to_path_buf(),
            stale_after_seconds,
        }
    }

    /// Task yang diklaim lalu "crash": attempt tanpa heartbeat selama 1 jam, memegang lease dan reservasi.
    async fn crashed(&self, id: &str, status: &str, worktree: bool) -> Uuid {
        TaskRepository::new(self.pool.clone())
            .create(&TaskContract {
                id: text(id),
                project_id: text(self.project.to_string()),
                project_run_id: text(self.run.to_string()),
                title: text("task"),
                role: text("worker"),
                objective: text("ship"),
                depends_on: vec![],
                allowed_paths: vec![AllowedPath::parse(format!("src/{id}/**")).unwrap()],
                context_refs: vec![],
                acceptance_criteria: vec![text("ok")],
                verification_commands: vec![text("cargo test")],
                limits: TaskLimits {
                    max_input_tokens: limit(100),
                    max_output_tokens: limit(50),
                    max_tool_calls: limit(5),
                    max_attempts: MaxAttempts::new(3).unwrap(),
                    timeout_seconds: limit(60),
                },
            })
            .await
            .unwrap();
        sqlx::query("UPDATE tasks SET status='READY' WHERE id=$1")
            .bind(id)
            .execute(&self.pool)
            .await
            .unwrap();
        let owner = Uuid::new_v4();
        let head = self.head.clone();
        let claimed = SchedulerStore::new(self.pool.clone())
            .claim_next(
                &ClaimRequest {
                    owner,
                    provider_id: "p",
                    model_id: "m",
                    retention_seconds: 60,
                    only_task: Some(id),
                },
                move |_| Some(head),
            )
            .await
            .unwrap()
            .unwrap();
        LeaseStore::new(self.pool.clone())
            .acquire(self.run, id, owner, &[&format!("src/{id}/**")], 600)
            .await
            .unwrap();
        BudgetStore::new(self.pool.clone())
            .reserve(&ReserveRequest {
                attempt_id: claimed.attempt.id,
                request_key: "k",
                input_tokens: 10,
                max_output_tokens: 5,
                purpose: Purpose::Work,
            })
            .await
            .unwrap();
        if worktree {
            self.manager
                .create(id, &claimed.attempt.branch, &self.head)
                .unwrap();
        }
        sqlx::query("UPDATE tasks SET status=$2 WHERE id=$1")
            .bind(id)
            .bind(status)
            .execute(&self.pool)
            .await
            .unwrap();
        self.age(claimed.attempt.id).await;
        claimed.attempt.id
    }

    async fn age(&self, attempt: Uuid) {
        sqlx::query("UPDATE agent_runs SET heartbeat_at=now()-interval '1 hour' WHERE id=$1")
            .bind(attempt)
            .execute(&self.pool)
            .await
            .unwrap();
    }

    async fn tool(&self, attempt: Uuid, call: &str, status: &str) {
        let query = if status == "completed" {
            "INSERT INTO tool_call_reservations (agent_run_id,call_id,status,outcome,duration_ms,completed_at,tool_name) VALUES ($1,$2,'completed','succeeded',5,now(),'apply_patch')"
        } else {
            "INSERT INTO tool_call_reservations (agent_run_id,call_id,status,tool_name) VALUES ($1,$2,'in_progress','apply_patch')"
        };
        sqlx::query(query)
            .bind(attempt)
            .bind(call)
            .execute(&self.pool)
            .await
            .unwrap();
    }

    async fn task_status(&self, id: &str) -> String {
        sqlx::query_scalar("SELECT status FROM tasks WHERE id=$1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn attempt(&self, attempt: Uuid) -> (String, Option<String>, bool) {
        let row: (String, Option<String>, bool) = sqlx::query_as(
            "SELECT status,error_code,finished_at IS NOT NULL FROM agent_runs WHERE id=$1",
        )
        .bind(attempt)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        row
    }

    async fn leases(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM file_leases WHERE project_run_id=$1")
            .bind(self.run)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn held(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM budget_reservations WHERE status='held'")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn recovery_events(&self, task: &str) -> Vec<serde_json::Value> {
        sqlx::query_scalar(
            "SELECT payload FROM events WHERE task_id=$1 AND event_type='recovery' ORDER BY id",
        )
        .bind(task)
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn crash_right_after_claim_requeues_instead_of_failing_startup(pool: PgPool) {
    // Regresi: attempt ASSIGNED tanpa worktree membuat recovery lama memaksa NEEDS_HUMAN pada transisi yang
    // tidak diizinkan (ASSIGNED + ambigu) sehingga startup gagal dengan error.
    let env = env(pool).await;
    let attempt = env.crashed("claimed", "ASSIGNED", false).await;

    let report = recover(&env.pool, &env.config(30)).await.unwrap();

    assert_eq!(report.requeued, ["claimed"]);
    assert_eq!(env.task_status("claimed").await, "READY");
    assert_eq!(
        env.attempt(attempt).await,
        ("failed".into(), Some("recovery.stale".into()), true)
    );
    assert_eq!(
        (env.leases().await, env.held().await),
        (0, 0),
        "lease dan reservasi dilepas"
    );
    assert_eq!(
        (
            report.leases_released,
            report.reservations_released,
            report.events
        ),
        (1, 1, 1)
    );
    // Attempt pengganti bisa langsung diklaim.
    let head = env.head.clone();
    let again = SchedulerStore::new(env.pool.clone())
        .claim_next(
            &ClaimRequest {
                owner: Uuid::new_v4(),
                provider_id: "p",
                model_id: "m",
                retention_seconds: 60,
                only_task: Some("claimed"),
            },
            move |_| Some(head),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again.attempt.attempt, 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn running_task_with_a_clean_worktree_is_requeued_and_the_worktree_discarded(pool: PgPool) {
    let env = env(pool).await;
    let attempt = env.crashed("running", "RUNNING", true).await;
    env.tool(attempt, "call-1", "completed").await; // hasil tercatat = bukti idempotensi
    let path = env.manager.worktree_root().join("running");
    assert!(path.exists());

    let report = recover(&env.pool, &env.config(30)).await.unwrap();

    assert_eq!(report.requeued, ["running"]);
    assert_eq!(env.task_status("running").await, "READY");
    assert!(
        !path.exists(),
        "worktree lama dibuang; attempt baru mulai bersih"
    );
    assert_eq!(env.leases().await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn unproven_side_effects_go_to_a_human_and_are_never_retried(pool: PgPool) {
    let env = env(pool).await;
    // Tool call masih in_progress: hasilnya tidak diketahui.
    let in_flight = env.crashed("in-flight", "RUNNING", true).await;
    env.tool(in_flight, "call-1", "in_progress").await;
    // Tahap INTEGRATE: patch mungkin sebagian diterapkan.
    let integrating = env.crashed("integrating", "INTEGRATE", true).await;
    // Worktree hilang saat RUNNING: pekerjaan tak bisa diperiksa.
    let vanished = env.crashed("vanished", "RUNNING", false).await;

    let report = recover(&env.pool, &env.config(30)).await.unwrap();

    assert!(report.requeued.is_empty());
    let mut reasons = report.needs_human.clone();
    reasons.sort();
    assert_eq!(
        reasons,
        [
            (
                "in-flight".to_owned(),
                "recovery.tool_in_progress".to_owned()
            ),
            (
                "integrating".to_owned(),
                "recovery.side_effect_ambiguous".to_owned()
            ),
            (
                "vanished".to_owned(),
                "recovery.worktree_unverified".to_owned()
            ),
        ]
    );
    for (task, attempt) in [
        ("in-flight", in_flight),
        ("integrating", integrating),
        ("vanished", vanished),
    ] {
        assert_eq!(env.task_status(task).await, "NEEDS_HUMAN", "{task}");
        assert_eq!(env.attempt(attempt).await.0, "recovery_required", "{task}");
        // Tidak ada attempt pengganti otomatis: task tidak READY, jadi tidak bisa diklaim scheduler.
        assert!(
            env.recovery_events(task)
                .await
                .iter()
                .all(|event| event["disposition"] == "recovery_required")
        );
    }
    let head = env.head.clone();
    assert!(
        SchedulerStore::new(env.pool.clone())
            .claim_next(
                &ClaimRequest {
                    owner: Uuid::new_v4(),
                    provider_id: "p",
                    model_id: "m",
                    retention_seconds: 60,
                    only_task: Some("in-flight")
                },
                move |_| Some(head)
            )
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        (env.leases().await, env.held().await),
        (0, 0),
        "lease dan reservasi tetap dilepas"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn recovery_events_record_what_was_decided(pool: PgPool) {
    let env = env(pool).await;
    let attempt = env.crashed("evented", "RUNNING", true).await;
    recover(&env.pool, &env.config(30)).await.unwrap();

    let events = env.recovery_events("evented").await;
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event["attempt_id"], attempt.to_string());
    assert_eq!(
        (
            event["disposition"].as_str(),
            event["previous_status"].as_str(),
            event["worktree"].as_str()
        ),
        (Some("requeued"), Some("RUNNING"), Some("cleaned"))
    );
    assert_eq!(
        (event["reason"].as_str(), event["leases_released"].as_i64()),
        (Some("recovery.stale"), Some(1))
    );
    let actor: String = sqlx::query_scalar(
        "SELECT actor_type FROM events WHERE event_type='recovery' AND task_id='evented'",
    )
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(actor, "system");
}

#[sqlx::test(migrations = "./migrations")]
async fn live_attempts_and_their_leases_are_left_alone(pool: PgPool) {
    let env = env(pool).await;
    let dead = env.crashed("dead", "RUNNING", true).await;
    let live = env.crashed("live", "RUNNING", true).await;
    sqlx::query("UPDATE agent_runs SET heartbeat_at=now() WHERE id=$1")
        .bind(live)
        .execute(&env.pool)
        .await
        .unwrap();

    let report = recover(&env.pool, &env.config(30)).await.unwrap();

    assert_eq!(report.requeued, ["dead"]);
    assert!(env.attempt(dead).await.2);
    assert_eq!(
        env.attempt(live).await,
        ("assigned".into(), None, false),
        "attempt segar tidak disentuh"
    );
    assert_eq!(env.task_status("live").await, "RUNNING");
    // Lease dan reservasi milik yang hidup tetap ada; hanya milik yang mati yang dilepas.
    let patterns: Vec<String> =
        sqlx::query_scalar("SELECT path_pattern FROM file_leases WHERE project_run_id=$1")
            .bind(env.run)
            .fetch_all(&env.pool)
            .await
            .unwrap();
    assert_eq!(patterns, ["src/live/**"]);
    assert_eq!(env.held().await, 1);
    assert!(env.manager.worktree_root().join("live").exists());
}

#[sqlx::test(migrations = "./migrations")]
async fn dead_leases_and_orphaned_reservations_are_swept(pool: PgPool) {
    let env = env(pool).await;
    let leases = LeaseStore::new(env.pool.clone());
    // Lease tanpa attempt yang tidak diperpanjang (crash di antara acquire dan claim) vs lease segar.
    let alive_attempt = env.crashed("alive", "RUNNING", false).await;
    sqlx::query("UPDATE agent_runs SET heartbeat_at=now() WHERE id=$1")
        .bind(alive_attempt)
        .execute(&env.pool)
        .await
        .unwrap();
    leases
        .acquire(env.run, "alive", Uuid::new_v4(), &["fresh/**"], 600)
        .await
        .unwrap();
    leases
        .acquire(env.run, "alive", Uuid::new_v4(), &["orphan/**"], 600)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE file_leases SET renewed_at=now()-interval '1 hour' WHERE path_pattern='orphan/**'",
    )
    .execute(&env.pool)
    .await
    .unwrap();
    // Reservasi tertahan milik attempt yang sudah selesai.
    let finished = env.crashed("finished", "DONE", false).await;
    sqlx::query("UPDATE agent_runs SET status='completed',finished_at=now() WHERE id=$1")
        .bind(finished)
        .execute(&env.pool)
        .await
        .unwrap();
    // Dikedaluwarsakan sesudah semua acquire: acquire membuang lease kedaluwarsa, jadi urutan ini penting.
    sqlx::query("UPDATE file_leases SET expires_at=now()-interval '1 second' WHERE path_pattern='src/alive/**'").execute(&env.pool).await.unwrap();
    sqlx::query("UPDATE project_runs SET status='RUNNING' WHERE id=$1")
        .bind(env.run)
        .execute(&env.pool)
        .await
        .unwrap();

    let report = recover(&env.pool, &env.config(30)).await.unwrap();

    assert_eq!(
        report.dead_leases_swept, 2,
        "yang tak diperpanjang dan yang kedaluwarsa"
    );
    assert_eq!(report.reservations_released, 1);
    let remaining: Vec<String> = sqlx::query_scalar(
        "SELECT path_pattern FROM file_leases WHERE project_run_id=$1 ORDER BY path_pattern",
    )
    .bind(env.run)
    .fetch_all(&env.pool)
    .await
    .unwrap();
    assert!(
        remaining.contains(&"fresh/**".to_owned()) && !remaining.contains(&"orphan/**".to_owned())
    );
    assert_eq!(
        env.held().await,
        1,
        "reservasi attempt yang masih hidup tidak dilepas"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn finished_tasks_with_unfinished_attempts_only_close_the_attempt(pool: PgPool) {
    let env = env(pool).await;
    let attempt = env.crashed("cancelled-task", "CANCELLED", false).await;

    let report = recover(&env.pool, &env.config(30)).await.unwrap();

    assert_eq!(report.closed, ["cancelled-task"]);
    assert_eq!(
        env.task_status("cancelled-task").await,
        "CANCELLED",
        "status task tidak diubah"
    );
    assert_eq!(
        env.attempt(attempt).await,
        ("failed".into(), Some("recovery.task_closed".into()), true)
    );
    assert_eq!(
        env.recovery_events("cancelled-task").await[0]["disposition"],
        "closed"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn recovery_is_idempotent_and_safe_to_run_concurrently(pool: PgPool) {
    let env = env(pool).await;
    for index in 0..4 {
        env.crashed(&format!("t{index}"), "RUNNING", true).await;
    }
    let config = env.config(30);
    let (a, b, c) = tokio::join!(
        recover(&env.pool, &config),
        recover(&env.pool, &config),
        recover(&env.pool, &config)
    );
    let total: usize = [a.unwrap(), b.unwrap(), c.unwrap()]
        .iter()
        .map(|report| report.requeued.len())
        .sum();
    assert_eq!(total, 4, "tiap attempt dipulihkan tepat sekali");
    for index in 0..4 {
        assert_eq!(env.recovery_events(&format!("t{index}")).await.len(), 1);
        assert_eq!(env.task_status(&format!("t{index}")).await, "READY");
    }
    // Sapuan kedua tidak melakukan apa pun.
    let again = recover(&env.pool, &config).await.unwrap();
    assert_eq!(
        (
            again.requeued.len(),
            again.needs_human.len(),
            again.events,
            again.leases_released
        ),
        (0, 0, 0, 0)
    );
}

// Deterministik: selama sesi lain memegang lock recovery (instance lain sedang menyapu), recover mengalah
// dan tidak menyentuh apa pun; setelah lock dilepas, sapuan berjalan normal.
#[sqlx::test(migrations = "./migrations")]
async fn recovery_yields_while_another_sweep_holds_the_lock(pool: PgPool) {
    let env = env(pool).await;
    let attempt = env.crashed("held-back", "RUNNING", true).await;

    let mut other = env.pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(RECOVERY_LOCK)
        .execute(&mut *other)
        .await
        .unwrap();
    let report = recover(&env.pool, &env.config(30)).await.unwrap();
    assert_eq!(
        (
            report.events,
            report.requeued.len(),
            report.leases_released,
            report.dead_leases_swept
        ),
        (0, 0, 0, 0)
    );
    assert!(
        !env.attempt(attempt).await.2,
        "attempt tidak disentuh selagi instance lain menyapu"
    );
    assert_eq!(env.task_status("held-back").await, "RUNNING");
    assert!(
        env.manager.worktree_root().join("held-back").exists(),
        "worktree tidak dibuang oleh pemulih yang mengalah"
    );

    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(RECOVERY_LOCK)
        .execute(&mut *other)
        .await
        .unwrap();
    drop(other);
    let report = recover(&env.pool, &env.config(30)).await.unwrap();
    assert_eq!(report.requeued, ["held-back"]);
    // Lock dilepas oleh recover sendiri: sapuan berikutnya tidak terblokir.
    assert_eq!(recover(&env.pool, &env.config(30)).await.unwrap().events, 0);
}
