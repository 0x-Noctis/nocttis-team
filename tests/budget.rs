// M4-003: budget guard hierarkis (src/domain/budget.rs, src/store/budget.rs).
use std::sync::Arc;

use ai_team::{
    domain::{
        budget::{BudgetLevel, Denial, Purpose},
        task::{AllowedPath, MaxAttempts, NonEmptyString, PositiveLimit, TaskContract, TaskLimits},
    },
    store::{
        budget::{BudgetError, BudgetStore, ReserveRequest},
        event::{AgentAttempt, Usage},
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

struct Fixture {
    pool: PgPool,
    project: Uuid,
    run: Uuid,
}

async fn fixture(pool: PgPool, budget: i64) -> Fixture {
    let (project, run) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name,repository_path) VALUES ($1,'project',$2)")
        .bind(project)
        .bind(format!("/repo/{project}"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'obj','RUNNING',$3)")
        .bind(run)
        .bind(project)
        .bind(budget)
        .execute(&pool)
        .await
        .unwrap();
    Fixture { pool, project, run }
}

impl Fixture {
    async fn task(&self, id: &str, max_input: i64, max_output: i64, max_attempts: i64) {
        TaskRepository::new(self.pool.clone())
            .create(&TaskContract {
                id: text(id),
                project_id: text(self.project.to_string()),
                project_run_id: text(self.run.to_string()),
                title: text("task"),
                role: text("worker"),
                objective: text("ship"),
                depends_on: vec![],
                allowed_paths: vec![AllowedPath::parse("src/**").unwrap()],
                context_refs: vec![],
                acceptance_criteria: vec![text("ok")],
                verification_commands: vec![text("cargo test")],
                limits: TaskLimits {
                    max_input_tokens: limit(max_input),
                    max_output_tokens: limit(max_output),
                    max_tool_calls: limit(5),
                    max_attempts: MaxAttempts::new(max_attempts).unwrap(),
                    timeout_seconds: limit(60),
                },
            })
            .await
            .unwrap();
    }

    async fn attempt(&self, task: &str, number: i64) -> Uuid {
        let id = Uuid::new_v4();
        TaskRepository::new(self.pool.clone())
            .create_attempt(&AgentAttempt {
                id,
                task_id: text(task),
                role: text("worker"),
                attempt: number,
                provider_id: text("provider"),
                model_id: text("model"),
                status: text("running"),
            })
            .await
            .unwrap();
        id
    }

    /// Task dengan batas longgar + satu attempt, untuk tes yang hanya menguji batas run.
    async fn loose_attempt(&self, id: &str) -> Uuid {
        self.task(id, 1_000_000, 1_000_000, 2).await;
        self.attempt(id, 1).await
    }

    async fn used(&self) -> i64 {
        BudgetStore::new(self.pool.clone())
            .run_view(self.run)
            .await
            .unwrap()
            .used
    }
}

fn request<'a>(
    attempt: Uuid,
    key: &'a str,
    input: i64,
    output: i64,
    purpose: Purpose,
) -> ReserveRequest<'a> {
    ReserveRequest {
        attempt_id: attempt,
        request_key: key,
        input_tokens: input,
        max_output_tokens: output,
        purpose,
    }
}

fn usage(input: i64, output: i64, estimated: bool) -> Usage {
    Usage {
        input_tokens: input,
        cached_tokens: 7,
        output_tokens: output,
        tool_calls: 1,
        latency_ms: 5,
        estimated,
    }
}

fn denial(result: Result<ai_team::store::budget::Reservation, BudgetError>) -> Denial {
    match result {
        Err(BudgetError::Denied(denial)) => denial,
        other => panic!("expected denial, got {other:?}"),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_reservations_never_oversubscribe_the_run(pool: PgPool) {
    // Budget 1.000 -> reserve 150 -> pekerjaan biasa maksimal 850 -> tepat 8 request @100 yang muat.
    let f = fixture(pool.clone(), 1_000).await;
    let mut attempts = Vec::new();
    for index in 0..20 {
        attempts.push(f.loose_attempt(&format!("t{index}")).await);
    }
    let store = Arc::new(BudgetStore::new(pool));
    let handles: Vec<_> = attempts
        .into_iter()
        .map(|attempt| {
            let store = store.clone();
            tokio::spawn(async move {
                store
                    .reserve(&request(attempt, "r1", 60, 40, Purpose::Work))
                    .await
                    .is_ok()
            })
        })
        .collect();
    let mut accepted = 0;
    for handle in handles {
        accepted += usize::from(handle.await.unwrap());
    }
    assert_eq!(accepted, 8);
    let view = store.run_view(f.run).await.unwrap();
    assert_eq!(
        (view.limit, view.reserve, view.held, view.used),
        (1_000, 150, 800, 0)
    );
    assert_eq!(view.level, BudgetLevel::Checkpoint); // 800/850
}

#[sqlx::test(migrations = "./migrations")]
async fn attempt_and_task_limits_apply_per_scope(pool: PgPool) {
    let f = fixture(pool.clone(), 1_000_000).await;
    // Attempt: input <= 400 dan output <= 100 (kumulatif); task: 2 attempt x 500 = 1.000.
    f.task("t", 400, 100, 2).await;
    let (first, second, third) = (
        f.attempt("t", 1).await,
        f.attempt("t", 2).await,
        f.attempt("t", 3).await,
    );
    let store = BudgetStore::new(pool);

    assert_eq!(
        denial(
            store
                .reserve(&request(first, "big-in", 401, 0, Purpose::Work))
                .await
        ),
        Denial::AttemptInput { remaining: 400 }
    );
    assert_eq!(
        denial(
            store
                .reserve(&request(first, "big-out", 0, 101, Purpose::Work))
                .await
        ),
        Denial::AttemptOutput { remaining: 100 }
    );
    let held = store
        .reserve(&request(first, "a", 300, 50, Purpose::Work))
        .await
        .unwrap();
    // Reservasi yang masih ditahan ikut dihitung terhadap batas attempt.
    assert_eq!(
        denial(
            store
                .reserve(&request(first, "b", 101, 0, Purpose::Work))
                .await
        ),
        Denial::AttemptInput { remaining: 100 }
    );
    // Settle dengan usage lebih kecil melepaskan selisihnya.
    store.settle(held.id, &usage(250, 40, false)).await.unwrap();
    let rest = store
        .reserve(&request(first, "b", 150, 60, Purpose::Work))
        .await
        .unwrap();
    store.settle(rest.id, &usage(150, 60, false)).await.unwrap(); // attempt 1: 400 in / 100 out

    let full = store
        .reserve(&request(second, "c", 400, 100, Purpose::Work))
        .await
        .unwrap();
    store
        .settle(full.id, &usage(400, 100, false))
        .await
        .unwrap(); // task: 1.000 = batas
    assert_eq!(
        denial(
            store
                .reserve(&request(third, "d", 1, 1, Purpose::Work))
                .await
        ),
        Denial::Task { remaining: 0 }
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn levels_warn_checkpoint_and_hard_stop_at_the_work_limit(pool: PgPool) {
    // Pekerjaan biasa: 850 token. 70% = 595, 85% = 722,5, 100% = 850.
    let f = fixture(pool.clone(), 1_000).await;
    let attempt = f.loose_attempt("t").await;
    let store = BudgetStore::new(pool);

    let mut seen = Vec::new();
    for (key, tokens) in [("a", 500), ("b", 95), ("c", 130), ("d", 125)] {
        let reservation = store
            .reserve(&request(attempt, key, tokens, 0, Purpose::Work))
            .await
            .unwrap();
        seen.push(reservation.levels.run);
        let after = store
            .settle(reservation.id, &usage(tokens, 0, false))
            .await
            .unwrap();
        assert_eq!(
            after.run, reservation.levels.run,
            "level setelah settle sama dengan saat reserve"
        );
    }
    // 500 -> Ok, 595 -> Warning (tepat 70%), 725 -> Checkpoint, 850 -> Stop (tepat 100%).
    assert_eq!(
        seen,
        [
            BudgetLevel::Ok,
            BudgetLevel::Warning,
            BudgetLevel::Checkpoint,
            BudgetLevel::Stop
        ]
    );
    assert_eq!(
        denial(
            store
                .reserve(&request(attempt, "e", 1, 0, Purpose::Work))
                .await
        ),
        Denial::Run { remaining: 0 }
    );
    assert_eq!(f.used().await, 850);
}

#[sqlx::test(migrations = "./migrations")]
async fn release_refunds_and_replays_are_idempotent(pool: PgPool) {
    let f = fixture(pool.clone(), 1_000).await;
    let attempt = f.loose_attempt("t").await;
    let other = f.loose_attempt("u").await;
    let store = BudgetStore::new(pool);

    let first = store
        .reserve(&request(attempt, "k", 100, 50, Purpose::Work))
        .await
        .unwrap();
    // Mengulang kunci yang sama mengembalikan reservasi yang sama dan tidak menahan token dua kali.
    let again = store
        .reserve(&request(attempt, "k", 100, 50, Purpose::Work))
        .await
        .unwrap();
    assert_eq!(first.id, again.id);
    assert_eq!(store.run_view(f.run).await.unwrap().held, 150);

    store.release(first.id).await.unwrap();
    store.release(first.id).await.unwrap(); // idempotent
    assert_eq!(store.run_view(f.run).await.unwrap().held, 0);
    assert!(matches!(
        store.settle(first.id, &usage(1, 1, false)).await,
        Err(BudgetError::Closed)
    ));
    // Kunci yang sudah ditutup tidak dipakai ulang; request baru memakai kunci baru.
    assert!(matches!(
        store
            .reserve(&request(attempt, "k", 1, 1, Purpose::Work))
            .await,
        Err(BudgetError::Closed)
    ));

    // Attempt yang mati melepaskan semua reservasinya sekaligus.
    store
        .reserve(&request(other, "x", 10, 10, Purpose::Work))
        .await
        .unwrap();
    store
        .reserve(&request(other, "y", 10, 10, Purpose::Work))
        .await
        .unwrap();
    assert_eq!(store.release_attempt(other).await.unwrap(), 2);
    assert_eq!(store.run_view(f.run).await.unwrap().held, 0);
    assert!(matches!(
        store.release(Uuid::new_v4()).await,
        Err(BudgetError::NotFound)
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn provider_without_usage_is_charged_as_a_conservative_estimate(pool: PgPool) {
    let f = fixture(pool.clone(), 10_000).await;
    let attempt = f.loose_attempt("t").await;
    let store = BudgetStore::new(pool.clone());

    // Provider tidak memberi usage: seluruh reservasi dibebankan dan ditandai estimasi.
    let reservation = store
        .reserve(&request(attempt, "est", 60, 40, Purpose::Work))
        .await
        .unwrap();
    store.settle_estimated(reservation.id).await.unwrap();
    store.settle_estimated(reservation.id).await.unwrap(); // idempotent: tidak ada baris usage kedua
    let view = store.run_view(f.run).await.unwrap();
    assert_eq!((view.used, view.held, view.estimated), (100, 0, true));
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM model_usage WHERE agent_run_id=$1")
        .bind(attempt)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 1);

    // Usage nyata: lebih kecil dari reservasi -> selisih dikembalikan; lebih besar -> dicatat apa adanya.
    let small = store
        .reserve(&request(attempt, "small", 100, 100, Purpose::Work))
        .await
        .unwrap();
    store.settle(small.id, &usage(30, 10, false)).await.unwrap();
    assert_eq!(f.used().await, 140);
    let over = store
        .reserve(&request(attempt, "over", 10, 10, Purpose::Work))
        .await
        .unwrap();
    store.settle(over.id, &usage(80, 50, false)).await.unwrap();
    assert_eq!(f.used().await, 270);
    assert!(
        store.settle(over.id, &usage(1, 1, false)).await.is_ok(),
        "settle ulang idempotent"
    );
    assert_eq!(f.used().await, 270);
}

#[sqlx::test(migrations = "./migrations")]
async fn recovery_may_use_the_reserve_but_not_beyond_the_run_budget(pool: PgPool) {
    let f = fixture(pool.clone(), 1_000).await;
    let attempt = f.loose_attempt("t").await;
    let store = BudgetStore::new(pool);
    let work = store
        .reserve(&request(attempt, "work", 850, 0, Purpose::Work))
        .await
        .unwrap();
    store.settle(work.id, &usage(850, 0, false)).await.unwrap();

    assert_eq!(
        denial(
            store
                .reserve(&request(attempt, "more", 1, 0, Purpose::Work))
                .await
        ),
        Denial::Run { remaining: 0 }
    );
    let recovery = store
        .reserve(&request(attempt, "rec1", 100, 0, Purpose::Recovery))
        .await
        .unwrap();
    store
        .settle(recovery.id, &usage(100, 0, false))
        .await
        .unwrap();
    let last = store
        .reserve(&request(attempt, "rec2", 50, 0, Purpose::Recovery))
        .await
        .unwrap();
    store.settle(last.id, &usage(50, 0, false)).await.unwrap();
    // 1.000/1.000: bahkan recovery berhenti di batas budget run.
    assert_eq!(
        denial(
            store
                .reserve(&request(attempt, "rec3", 1, 0, Purpose::Recovery))
                .await
        ),
        Denial::Run { remaining: 0 }
    );
    assert_eq!(f.used().await, 1_000);
}

#[sqlx::test(migrations = "./migrations")]
async fn invalid_or_closed_subjects_are_rejected(pool: PgPool) {
    let f = fixture(pool.clone(), 1_000).await;
    let attempt = f.loose_attempt("t").await;
    let store = BudgetStore::new(pool.clone());

    assert!(matches!(
        store
            .reserve(&request(Uuid::new_v4(), "k", 1, 1, Purpose::Work))
            .await,
        Err(BudgetError::NotFound)
    ));
    assert!(matches!(
        store
            .reserve(&request(attempt, " ", 1, 1, Purpose::Work))
            .await,
        Err(BudgetError::InvalidInput("request_key"))
    ));
    assert!(matches!(
        store
            .reserve(&request(attempt, "k", -1, 1, Purpose::Work))
            .await,
        Err(BudgetError::InvalidInput("tokens"))
    ));
    let held = store
        .reserve(&request(attempt, "ok", 10, 10, Purpose::Work))
        .await
        .unwrap();
    assert!(matches!(
        store.settle(held.id, &usage(-1, 0, false)).await,
        Err(BudgetError::InvalidInput("usage"))
    ));

    sqlx::query("UPDATE agent_runs SET status='failed',finished_at=now() WHERE id=$1")
        .bind(attempt)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store
            .reserve(&request(attempt, "late", 1, 1, Purpose::Work))
            .await,
        Err(BudgetError::Closed)
    ));
}

// Spesifikasi serialisasi yang deterministik: selama transaksi lain memegang baris run, reserve menunggu.
// FOR NO KEY UPDATE dipakai karena tidak bentrok dengan FOR KEY SHARE milik FK insert reservasi; jadi
// hanya `FOR UPDATE` eksplisit di kode produksi yang bisa membuat reserve menunggu.
#[sqlx::test(migrations = "./migrations")]
async fn reserve_waits_for_the_run_row_lock(pool: PgPool) {
    let f = fixture(pool.clone(), 1_000).await;
    let attempt = f.loose_attempt("t").await;
    let store = Arc::new(BudgetStore::new(pool.clone()));

    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM project_runs WHERE id=$1 FOR NO KEY UPDATE")
        .bind(f.run)
        .execute(&mut *holder)
        .await
        .unwrap();
    let waiting = {
        let store = store.clone();
        tokio::spawn(async move {
            store
                .reserve(&request(attempt, "k", 10, 10, Purpose::Work))
                .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert!(!waiting.is_finished(), "reserve harus menunggu kunci run");
    holder.rollback().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
        .await
        .expect("lanjut setelah kunci dilepas")
        .unwrap()
        .unwrap();
}
