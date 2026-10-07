// M5-002: retensi artifact yatim dan worktree cabang integrasi; waktu dikendalikan lewat `now` dan mtime file.
use std::{
    fs::{self, File},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime},
};

use ai_team::{
    domain::task::NonEmptyString,
    retention::{ActionKind, Outcome, RetentionConfig, run},
    runner::{git::GitWorktreeManager, integration_git::IntegrationBranch},
    store::{
        artifact::ArtifactStore,
        event::{IntegrationOperation, IntegrationStatus},
        task::TaskRepository,
    },
};
use sqlx::PgPool;
use uuid::Uuid;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
const DAY: Duration = Duration::from_secs(24 * 3600);

struct Fixture {
    root: PathBuf,
    repository: PathBuf,
    worktrees: PathBuf,
    artifacts: ArtifactStore,
    base: String,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "noctis-retention-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let repository = root.join("repository");
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
        let artifacts = ArtifactStore::new(root.join("artifacts"), 1024).unwrap();
        Self {
            worktrees: root.join("worktrees"),
            repository,
            artifacts,
            base,
            root,
        }
    }

    fn config(&self) -> RetentionConfig {
        RetentionConfig {
            worktree_root: self.worktrees.clone(),
            retention: DAY,
        }
    }

    /// Tulis artifact lalu set mtime kedua file-nya `age` ke belakang.
    fn artifact(&self, id: &str, age: Duration) {
        self.artifacts
            .write(id, "f.txt", "text/plain", b"data", |_| Ok::<_, ()>(()))
            .unwrap();
        let when = SystemTime::now() - age;
        for suffix in [".artifact", ".metadata.json"] {
            File::options()
                .write(true)
                .open(self.root.join("artifacts").join(format!("{id}{suffix}")))
                .unwrap()
                .set_modified(when)
                .unwrap();
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let dir = self.root.join("artifacts");
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn git(path: &Path, arguments: &[&str]) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(arguments)
            .status()
            .unwrap()
            .success()
    );
}

fn git_output(path: &Path, arguments: &[&str]) -> String {
    String::from_utf8(
        Command::new("git")
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

async fn seed_run(pool: &PgPool, repository: &Path) -> Uuid {
    let run = Uuid::new_v4();
    // Beberapa run boleh berbagi satu project (repository_path unik).
    let project: Uuid = sqlx::query_scalar("INSERT INTO projects (id,name,repository_path) VALUES ($1,$2,$3) ON CONFLICT (repository_path) DO UPDATE SET name=projects.name RETURNING id")
        .bind(Uuid::new_v4())
        .bind(format!("retention-{run}"))
        .bind(repository.to_string_lossy().as_ref())
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'retention','RUNNING',100000)")
        .bind(run)
        .bind(project)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO providers (id,base_url,api_key_env,request_timeout_seconds) VALUES ($1,'http://127.0.0.1:1','X',1)")
        .bind(format!("p-{run}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO models (id,provider_id,remote_name,class,context_window,max_output_tokens) VALUES ($1,$2,'m','coding',1000,1000)")
        .bind(format!("m-{run}"))
        .bind(format!("p-{run}"))
        .execute(pool)
        .await
        .unwrap();
    run
}

async fn seed_task(pool: &PgPool, run: Uuid, id: &str, status: &str, age_hours: i32) -> Uuid {
    sqlx::query("INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,max_attempts,updated_at) VALUES ($1,$2,'worker','t','o',$3,'[\"tracked.txt\"]','[\"ok\"]','[\"true\"]',1000,1000,2, now() - make_interval(hours => $4))")
        .bind(id)
        .bind(run)
        .bind(status)
        .bind(age_hours)
        .execute(pool)
        .await
        .unwrap();
    let attempt = Uuid::new_v4();
    sqlx::query("INSERT INTO agent_runs (id,task_id,role,provider_id,model_id,attempt,status,branch,base_commit,heartbeat_at,finished_at,retain_until) VALUES ($1,$2,'worker',$3,$4,1,'completed',$5,$6,now(),now(),now())")
        .bind(attempt)
        .bind(id)
        .bind(format!("p-{run}"))
        .bind(format!("m-{run}"))
        .bind(format!("noctis-{id}"))
        .bind("0".repeat(40))
        .execute(pool)
        .await
        .unwrap();
    attempt
}

fn text(value: &str) -> NonEmptyString {
    NonEmptyString::parse("v", value).unwrap()
}

/// Run dengan satu task berstatus `status`, cabang integrasi nyata, dan catatan operasi integrasi.
async fn integration_run(pool: &PgPool, fixture: &Fixture, status: &str, age_hours: i32) -> Uuid {
    let run = seed_run(pool, &fixture.repository).await;
    let task = format!("task-{run}");
    let attempt = seed_task(pool, run, &task, status, age_hours).await;
    let manager = GitWorktreeManager::new(&fixture.repository, &fixture.worktrees).unwrap();
    let branch = IntegrationBranch::create(&manager, &run.to_string(), &fixture.base).unwrap();
    TaskRepository::new(pool.clone())
        .prepare_integration(&IntegrationOperation {
            id: Uuid::new_v4(),
            attempt_id: attempt,
            task_id: text(&task),
            source_branch: format!("noctis-{task}"),
            source_base_commit: fixture.base.clone(),
            target_id: format!("integration-{run}"),
            target_branch: branch.branch().to_owned(),
            target_base_commit: fixture.base.clone(),
            patch_sha256: "0".repeat(64),
            owner_token: Uuid::new_v4(),
            status: IntegrationStatus::Completed,
        })
        .await
        .unwrap();
    run
}

async fn audit_rows(pool: &PgPool) -> Vec<(bool, String, String, String)> {
    sqlx::query_as("SELECT dry_run,kind,target,outcome FROM retention_actions ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn orphans_are_deleted_but_young_and_referenced_artifacts_survive(pool: PgPool) {
    let fixture = Fixture::new();
    let run = seed_run(&pool, &fixture.repository).await;
    let attempt = seed_task(&pool, run, "ref-task", "DONE", 100).await;
    let in_table = Uuid::new_v4().to_string();
    let in_tool = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO artifacts (id,task_id,kind,logical_name,media_type,size_bytes,sha256) VALUES ($1::uuid,'ref-task','diff','official.diff','text/x-diff',4,$2)")
        .bind(&in_table)
        .bind("0".repeat(64))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO tool_call_reservations (agent_run_id,call_id,status,outcome,duration_ms,artifact_id,completed_at,tool_name) VALUES ($1,'c1','completed','succeeded',1,$2::uuid,now(),'read_file')")
        .bind(attempt)
        .bind(&in_tool)
        .execute(&pool)
        .await
        .unwrap();
    // Bukti verifikasi dirujuk lewat payload event, bukan tabel artifacts.
    sqlx::query("INSERT INTO events (project_run_id,task_id,actor_type,event_type,payload) VALUES ($1,'ref-task','system','verification','{\"artifact_ids\":[\"verify-abc-stdout\",\"verify-abc-stderr\"]}')")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    for id in [
        "orphan-old",
        "verify-abc-stdout",
        "verify-abc-stderr",
        &in_table,
        &in_tool,
    ] {
        fixture.artifact(id, 3 * DAY);
    }
    fixture.artifact("orphan-young", Duration::from_secs(60));

    let report = run_retention(&pool, &fixture, false).await;
    assert_eq!(
        report
            .actions
            .iter()
            .map(|a| (a.target.as_str(), a.outcome))
            .collect::<Vec<_>>(),
        [("orphan-old", Outcome::Deleted)]
    );
    assert!(!fixture.artifacts.contains("orphan-old"));
    for kept in [
        "orphan-young",
        "verify-abc-stdout",
        "verify-abc-stderr",
        &in_table,
        &in_tool,
    ] {
        assert!(
            fixture.artifacts.contains(kept),
            "{kept} tidak boleh terhapus"
        );
    }
    assert_eq!(
        audit_rows(&pool).await,
        [(
            false,
            "orphan_artifact".into(),
            "orphan-old".into(),
            "deleted".into()
        )]
    );
    // Idempoten: putaran kedua tidak punya pekerjaan dan tidak menambah audit.
    assert!(
        run_retention(&pool, &fixture, false)
            .await
            .actions
            .is_empty()
    );
    assert_eq!(audit_rows(&pool).await.len(), 1);
}

async fn run_retention(
    pool: &PgPool,
    fixture: &Fixture,
    dry_run: bool,
) -> ai_team::retention::Report {
    run(
        pool,
        &fixture.artifacts,
        &fixture.config(),
        dry_run,
        SystemTime::now(),
    )
    .await
    .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn dry_run_reports_and_audits_without_deleting(pool: PgPool) {
    let fixture = Fixture::new();
    fixture.artifact("orphan-old", 3 * DAY);
    let run_id = integration_run(&pool, &fixture, "DONE", 100).await;

    let report = run_retention(&pool, &fixture, true).await;
    assert!(report.dry_run);
    let mut kinds: Vec<_> = report.actions.iter().map(|a| (a.kind, a.outcome)).collect();
    kinds.sort_by_key(|(kind, _)| *kind as u8);
    assert_eq!(
        kinds,
        [
            (ActionKind::OrphanArtifact, Outcome::WouldDelete),
            (ActionKind::IntegrationWorktree, Outcome::WouldDelete)
        ]
    );
    assert!(fixture.artifacts.contains("orphan-old"));
    assert!(
        fixture
            .worktrees
            .join(format!("integration-{run_id}"))
            .exists()
    );
    assert!(
        audit_rows(&pool)
            .await
            .iter()
            .all(|row| row.0 && row.3 == "would_delete")
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn failed_delete_is_reported_and_succeeds_on_retry(pool: PgPool) {
    let fixture = Fixture::new();
    fixture.artifact("orphan-old", 3 * DAY);
    let dir = fixture.root.join("artifacts");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    // Bila proses uji kebal izin (mis. root), kegagalan tidak bisa dipaksakan.
    if fs::write(dir.join("probe"), b"x").is_ok() {
        return;
    }

    let report = run_retention(&pool, &fixture, false).await;
    assert_eq!(report.actions[0].outcome, Outcome::Failed);
    assert!(fixture.artifacts.contains("orphan-old"));

    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    let retry = run_retention(&pool, &fixture, false).await;
    assert_eq!(retry.actions[0].outcome, Outcome::Deleted);
    assert!(!fixture.artifacts.contains("orphan-old"));
    let outcomes: Vec<_> = audit_rows(&pool)
        .await
        .into_iter()
        .map(|row| row.3)
        .collect();
    assert_eq!(outcomes, ["failed", "deleted"]);
}

#[sqlx::test(migrations = "./migrations")]
async fn database_rows_without_files_are_reported_never_deleted(pool: PgPool) {
    let fixture = Fixture::new();
    let run = seed_run(&pool, &fixture.repository).await;
    seed_task(&pool, run, "t1", "DONE", 100).await;
    let ghost = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO artifacts (id,task_id,kind,logical_name,media_type,size_bytes,sha256) VALUES ($1::uuid,'t1','diff','official.diff','text/x-diff',4,$2)")
        .bind(&ghost)
        .bind("0".repeat(64))
        .execute(&pool)
        .await
        .unwrap();
    let report = run_retention(&pool, &fixture, false).await;
    assert_eq!(report.missing_files, [ghost]);
    assert!(report.actions.is_empty());
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM artifacts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn finished_run_worktree_is_removed_but_branch_and_open_runs_are_kept(pool: PgPool) {
    let fixture = Fixture::new();
    let done_old = integration_run(&pool, &fixture, "DONE", 100).await;
    let done_young = integration_run(&pool, &fixture, "DONE", 1).await;
    let needs_human = integration_run(&pool, &fixture, "NEEDS_HUMAN", 100).await;

    let report = run_retention(&pool, &fixture, false).await;
    assert_eq!(
        report
            .actions
            .iter()
            .map(|a| (a.kind, a.target.clone(), a.outcome))
            .collect::<Vec<_>>(),
        [(
            ActionKind::IntegrationWorktree,
            done_old.to_string(),
            Outcome::Deleted
        )]
    );
    let dir = |run: Uuid| fixture.worktrees.join(format!("integration-{run}"));
    assert!(!dir(done_old).exists());
    assert!(
        dir(done_young).exists(),
        "run baru selesai harus dipertahankan"
    );
    assert!(
        dir(needs_human).exists(),
        "run yang menunggu manusia harus dipertahankan"
    );
    // Hasil kerja tetap ada sebagai branch untuk merge manual.
    assert!(
        !git_output(
            &fixture.repository,
            &[
                "branch",
                "--list",
                &format!("noctis-integration-{done_old}")
            ]
        )
        .is_empty()
    );
    assert_eq!(
        git_output(&fixture.repository, &["rev-parse", "main"]),
        fixture.base
    );
    // Putaran berikutnya tidak mengulang pekerjaan yang sudah selesai.
    assert!(
        run_retention(&pool, &fixture, false)
            .await
            .actions
            .is_empty()
    );
}
