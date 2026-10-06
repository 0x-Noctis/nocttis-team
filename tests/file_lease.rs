// M4-002: file lease dan deteksi overlap (src/store/lease.rs, src/domain/path_scope.rs).
use std::sync::Arc;

use ai_team::{
    domain::task::{
        AllowedPath, MaxAttempts, NonEmptyString, PositiveLimit, TaskContract, TaskLimits,
    },
    store::{
        lease::{LeaseError, LeaseStore},
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

/// Project + run, dan task-task kosong (hanya perlu ada karena `file_leases.task_id` adalah FK).
async fn run_with_tasks(pool: &PgPool, tasks: &[&str]) -> Uuid {
    let (project, run) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name,repository_path) VALUES ($1,'project',$2)")
        .bind(project)
        .bind(format!("/repo/{project}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'obj','RUNNING',1000000)")
        .bind(run)
        .bind(project)
        .execute(pool)
        .await
        .unwrap();
    add_tasks(pool, project, run, tasks).await;
    run
}

async fn add_tasks(pool: &PgPool, project: Uuid, run: Uuid, tasks: &[&str]) {
    let repository = TaskRepository::new(pool.clone());
    for id in tasks {
        repository
            .create(&TaskContract {
                id: text(*id),
                project_id: text(project.to_string()),
                project_run_id: text(run.to_string()),
                title: text("task"),
                role: text("worker"),
                objective: text("ship"),
                depends_on: vec![],
                allowed_paths: vec![AllowedPath::parse("src/**").unwrap()],
                context_refs: vec![],
                acceptance_criteria: vec![text("ok")],
                verification_commands: vec![text("cargo test")],
                limits: TaskLimits {
                    max_input_tokens: limit(10),
                    max_output_tokens: limit(10),
                    max_tool_calls: limit(1),
                    max_attempts: MaxAttempts::new(2).unwrap(),
                    timeout_seconds: limit(10),
                },
            })
            .await
            .unwrap();
    }
}

fn conflict_with(result: Result<Vec<String>, LeaseError>) -> (String, String) {
    match result {
        Err(LeaseError::Conflict {
            holder_task_id,
            pattern,
        }) => (holder_task_id, pattern),
        other => panic!("expected conflict, got {other:?}"),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn exact_conflict_is_rejected_atomically(pool: PgPool) {
    let run = run_with_tasks(&pool, &["a", "b"]).await;
    let leases = LeaseStore::new(pool.clone());
    let (alice, bob) = (Uuid::new_v4(), Uuid::new_v4());
    leases
        .acquire(run, "a", alice, &["src/a.rs"], 60)
        .await
        .unwrap();

    // Satu pola bebas dan satu bentrok: tidak boleh ada yang tersimpan (all-or-nothing).
    let result = leases
        .acquire(run, "b", bob, &["docs/x.md", "./SRC//a.rs"], 60)
        .await;
    assert_eq!(
        conflict_with(result),
        ("a".to_owned(), "src/a.rs".to_owned())
    );
    let held: Vec<_> = leases
        .active(run)
        .await
        .unwrap()
        .into_iter()
        .map(|lease| lease.pattern)
        .collect();
    assert_eq!(held, ["src/a.rs"]);

    // Setelah dilepas, scope yang sama bisa diambil.
    assert_eq!(leases.release(run, alice).await.unwrap(), 1);
    assert_eq!(
        leases
            .acquire(run, "b", bob, &["src/a.rs"], 60)
            .await
            .unwrap(),
        ["src/a.rs"]
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn glob_overlap_is_detected_conservatively(pool: PgPool) {
    let run = run_with_tasks(&pool, &["a", "b", "c", "d", "e"]).await;
    let leases = LeaseStore::new(pool);
    leases
        .acquire(run, "a", Uuid::new_v4(), &["src/**"], 60)
        .await
        .unwrap();

    for (task, pattern) in [
        ("b", "src/api/x.rs"),
        ("c", "src/*/mod.rs"),
        ("d", "**/*.rs"),
        ("e", "SRC/"),
    ] {
        let result = leases
            .acquire(run, task, Uuid::new_v4(), &[pattern], 60)
            .await;
        assert_eq!(
            conflict_with(result).0,
            "a",
            "{pattern} harus bentrok dengan src/**"
        );
    }
    // Direktori bersaudara dan awalan yang bukan batas komponen tidak bentrok.
    leases
        .acquire(
            run,
            "b",
            Uuid::new_v4(),
            &["tests/**", "src2/lib.rs", "docs/*.md"],
            60,
        )
        .await
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn same_owner_reacquires_but_other_owner_of_same_task_conflicts(pool: PgPool) {
    let run = run_with_tasks(&pool, &["a"]).await;
    let leases = LeaseStore::new(pool.clone());
    let (attempt1, attempt2) = (Uuid::new_v4(), Uuid::new_v4());
    leases
        .acquire(run, "a", attempt1, &["src/**"], 60)
        .await
        .unwrap();
    // Idempotent dan boleh melebar ke scope yang beririsan dengan miliknya sendiri.
    leases
        .acquire(run, "a", attempt1, &["src/**", "src/api/x.rs"], 60)
        .await
        .unwrap();
    assert_eq!(leases.active(run).await.unwrap().len(), 2);
    // Attempt pengganti dari TASK YANG SAMA tidak boleh menyelinap sebelum attempt lama dilepas.
    assert_eq!(
        conflict_with(
            leases
                .acquire(run, "a", attempt2, &["src/api/x.rs"], 60)
                .await
        )
        .0,
        "a"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_acquires_have_exactly_one_winner(pool: PgPool) {
    let ids: Vec<String> = (0..10).map(|index| format!("t{index}")).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let run = run_with_tasks(&pool, &refs).await;
    let leases = Arc::new(LeaseStore::new(pool.clone()));
    // Pola berbeda tetapi semuanya beririsan: lewat unique constraint saja tidak akan tertangkap.
    let patterns = ["src/**", "src/a.rs", "src/*/b.rs", "**/*.rs", "src/"];
    let handles: Vec<_> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let (leases, id) = (leases.clone(), id.clone());
            let pattern = patterns[index % patterns.len()];
            tokio::spawn(async move {
                leases
                    .acquire(run, &id, Uuid::new_v4(), &[pattern], 60)
                    .await
                    .is_ok()
            })
        })
        .collect();
    let mut winners = 0;
    for handle in handles {
        winners += usize::from(handle.await.unwrap());
    }
    assert_eq!(winners, 1);
    assert_eq!(leases.active(run).await.unwrap().len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn runs_do_not_block_each_other(pool: PgPool) {
    let first = run_with_tasks(&pool, &["r1"]).await;
    let second = run_with_tasks(&pool, &["r2"]).await;
    let leases = LeaseStore::new(pool);
    leases
        .acquire(first, "r1", Uuid::new_v4(), &["src/**"], 60)
        .await
        .unwrap();
    leases
        .acquire(second, "r2", Uuid::new_v4(), &["src/**"], 60)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn expired_lease_can_be_taken_and_old_owner_loses_it(pool: PgPool) {
    let run = run_with_tasks(&pool, &["a", "b"]).await;
    let leases = LeaseStore::new(pool.clone());
    let (alice, bob) = (Uuid::new_v4(), Uuid::new_v4());
    leases
        .acquire(run, "a", alice, &["src/**"], 60)
        .await
        .unwrap();
    assert_eq!(
        conflict_with(leases.acquire(run, "b", bob, &["src/x.rs"], 60).await).0,
        "a"
    );

    // Alice berhenti memperpanjang; lease kedaluwarsa -> Bob merebutnya.
    sqlx::query("UPDATE file_leases SET expires_at=now()-interval '1 second' WHERE owner=$1")
        .bind(alice)
        .execute(&pool)
        .await
        .unwrap();
    assert!(leases.active(run).await.unwrap().is_empty());
    leases
        .acquire(run, "b", bob, &["src/x.rs"], 60)
        .await
        .unwrap();

    // Alice sadar sudah kehilangan lease; renew gagal dan release tidak menyentuh milik Bob.
    assert!(matches!(
        leases.renew(run, alice, 60).await,
        Err(LeaseError::Lost)
    ));
    assert_eq!(leases.release(run, alice).await.unwrap(), 0);
    assert_eq!(leases.active(run).await.unwrap()[0].owner, bob);
    assert_eq!(leases.renew(run, bob, 120).await.unwrap(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn recovery_can_release_a_dead_attempts_leases(pool: PgPool) {
    let run = run_with_tasks(&pool, &["a", "b"]).await;
    let leases = LeaseStore::new(pool);
    leases
        .acquire(run, "a", Uuid::new_v4(), &["src/**", "docs/**"], 600)
        .await
        .unwrap();
    assert_eq!(
        conflict_with(
            leases
                .acquire(run, "b", Uuid::new_v4(), &["docs/x.md"], 60)
                .await
        )
        .0,
        "a"
    );
    // Recovery menyatakan attempt mati -> lease dilepas tanpa menunggu 10 menit.
    assert_eq!(leases.release_task("a").await.unwrap(), 2);
    leases
        .acquire(run, "b", Uuid::new_v4(), &["docs/x.md"], 60)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn invalid_requests_change_nothing(pool: PgPool) {
    let run = run_with_tasks(&pool, &["a"]).await;
    let leases = LeaseStore::new(pool);
    let owner = Uuid::new_v4();
    for patterns in [&["../x"][..], &["/etc/passwd"], &[""], &["ok.rs", "a/../b"]] {
        assert!(
            matches!(
                leases.acquire(run, "a", owner, patterns, 60).await,
                Err(LeaseError::InvalidScope(_))
            ),
            "{patterns:?}"
        );
    }
    assert!(matches!(
        leases.acquire(run, "a", owner, &[], 60).await,
        Err(LeaseError::InvalidInput("patterns"))
    ));
    assert!(matches!(
        leases.acquire(run, "a", owner, &["x"], 0).await,
        Err(LeaseError::InvalidInput("ttl_seconds"))
    ));
    assert!(matches!(
        leases.acquire(run, "a", owner, &["x"], 7_200).await,
        Err(LeaseError::InvalidInput("ttl_seconds"))
    ));
    assert!(matches!(
        leases.acquire(run, " ", owner, &["x"], 60).await,
        Err(LeaseError::InvalidInput("task_id"))
    ));
    assert!(leases.active(run).await.unwrap().is_empty());
}

// Cek-lalu-simpan harus diserialkan per run: selama transaksi lain memegang lock run, acquire menunggu;
// run lain tidak terpengaruh.
#[sqlx::test(migrations = "./migrations")]
async fn acquire_is_serialized_per_run_by_advisory_lock(pool: PgPool) {
    let run = run_with_tasks(&pool, &["a"]).await;
    let other_run = run_with_tasks(&pool, &["z"]).await;
    let leases = Arc::new(LeaseStore::new(pool.clone()));

    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('file_leases:' || $1::text, 0))")
        .bind(run)
        .execute(&mut *holder)
        .await
        .unwrap();
    let waiting = {
        let leases = leases.clone();
        tokio::spawn(async move {
            leases
                .acquire(run, "a", Uuid::new_v4(), &["src/**"], 60)
                .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert!(!waiting.is_finished(), "acquire harus menunggu lock run");
    // Run lain tetap berjalan tanpa menunggu.
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        leases.acquire(other_run, "z", Uuid::new_v4(), &["src/**"], 60),
    )
    .await
    .expect("run lain tidak boleh menunggu")
    .unwrap();
    holder.rollback().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
        .await
        .expect("acquire harus lanjut setelah lock dilepas")
        .unwrap()
        .unwrap();
}
