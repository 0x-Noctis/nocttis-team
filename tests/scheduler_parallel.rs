// M4-005: scheduler paralel (src/scheduler/parallel.rs) dengan runner palsu berpagar (gate), tanpa model,
// Git, atau Docker. Gate membuat setiap skenario deterministik: task "berjalan" sampai test melepasnya.
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use ai_team::{
    domain::{
        path_scope::PathScope,
        task::{AllowedPath, MaxAttempts, NonEmptyString, PositiveLimit, TaskContract, TaskLimits},
    },
    orchestrator::scheduler::parallel::{
        ParallelConfig, ParallelScheduler, RunOutcome, SchedulerError, Skip, SlotControl,
        SlotRunner, StopReason,
    },
    store::{
        event::{AgentAttempt, Usage},
        lease::LeaseStore,
        scheduler::{ClaimRequest, ClaimedTask, SchedulerStore},
        task::TaskRepository,
    },
};
use sqlx::PgPool;
use tokio::sync::Notify;
use uuid::Uuid;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn text(value: impl Into<String>) -> NonEmptyString {
    NonEmptyString::parse("value", value).unwrap()
}

fn limit(value: i64) -> PositiveLimit {
    PositiveLimit::new("limit", value).unwrap()
}

fn config(slots: usize) -> ParallelConfig {
    ParallelConfig {
        slots,
        provider_id: "provider".into(),
        model_id: "model".into(),
        retention_seconds: 60,
        lease_ttl_seconds: 30,
        heartbeat_interval: Duration::from_millis(40),
        stale_after_seconds: 5,
        idle_min: Duration::from_millis(10),
        idle_max: Duration::from_millis(50),
        shutdown_grace: Duration::from_millis(300),
        candidate_window: 32,
        recover_every: Duration::from_millis(100),
    }
}

/// Task yang sedang berjalan: (task, run, scope-nya).
type RunningSet = Vec<(String, Uuid, Vec<PathScope>)>;

/// Runner palsu: mencatat start/stop, menahan task bergerbang sampai `release`, dan menandai task DONE.
#[derive(Clone)]
struct FakeRunner {
    pool: PgPool,
    gates: Arc<Mutex<HashMap<String, Arc<Notify>>>>,
    started: Arc<Mutex<Vec<String>>>,
    stopped: Arc<Mutex<Vec<(String, StopReason)>>>,
    running: Arc<Mutex<RunningSet>>,
    peak: Arc<AtomicUsize>,
    overlaps: Arc<AtomicUsize>,
}

impl FakeRunner {
    fn new(pool: PgPool) -> Self {
        Self {
            pool,
            gates: Arc::default(),
            started: Arc::default(),
            stopped: Arc::default(),
            running: Arc::default(),
            peak: Arc::default(),
            overlaps: Arc::default(),
        }
    }

    fn gate(&self, ids: &[&str]) {
        for id in ids {
            self.gates
                .lock()
                .unwrap()
                .insert((*id).to_owned(), Arc::new(Notify::new()));
        }
    }

    fn release(&self, id: &str) {
        let gate = self
            .gates
            .lock()
            .unwrap()
            .remove(id)
            .expect("task has a gate");
        gate.notify_one();
    }

    fn release_all(&self) {
        let ids: Vec<String> = self.gates.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.release(&id);
        }
    }

    fn started(&self) -> Vec<String> {
        self.started.lock().unwrap().clone()
    }

    fn stopped(&self) -> Vec<(String, StopReason)> {
        self.stopped.lock().unwrap().clone()
    }

    fn leave(&self, id: &str) {
        self.running
            .lock()
            .unwrap()
            .retain(|(task, _, _)| task != id);
    }
}

impl SlotRunner for FakeRunner {
    fn run(
        &self,
        task: &ClaimedTask,
        mut control: SlotControl,
    ) -> impl Future<Output = RunOutcome> + Send {
        let this = self.clone();
        let id = task.attempt.task_id.as_str().to_owned();
        let run = task.run_id;
        async move {
            let paths: Vec<String> = serde_json::from_value(
                sqlx::query_scalar("SELECT allowed_paths FROM tasks WHERE id=$1")
                    .bind(&id)
                    .fetch_one(&this.pool)
                    .await
                    .unwrap(),
            )
            .unwrap();
            let scopes: Vec<PathScope> = paths
                .iter()
                .map(|path| PathScope::parse(path).unwrap())
                .collect();
            {
                // Dua task satu run yang scope-nya beririsan tidak boleh pernah berjalan bersamaan.
                let mut running = this.running.lock().unwrap();
                for (_, other_run, other) in running.iter() {
                    if *other_run == run
                        && other.iter().any(|a| scopes.iter().any(|b| a.overlaps(b)))
                    {
                        this.overlaps.fetch_add(1, Ordering::SeqCst);
                    }
                }
                running.push((id.clone(), run, scopes));
                this.peak.fetch_max(running.len(), Ordering::SeqCst);
            }
            this.started.lock().unwrap().push(id.clone());
            let gate = this.gates.lock().unwrap().get(&id).cloned();
            if let Some(gate) = gate {
                tokio::select! {
                    () = gate.notified() => {}
                    reason = control.stopped() => {
                        this.stopped.lock().unwrap().push((id.clone(), reason));
                        this.leave(&id);
                        return RunOutcome::Stopped;
                    }
                }
            }
            sqlx::query("UPDATE tasks SET status='DONE' WHERE id=$1")
                .bind(&id)
                .execute(&this.pool)
                .await
                .unwrap();
            this.leave(&id);
            RunOutcome::Finished
        }
    }
}

async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    for _ in 0..300 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timeout waiting for: {what}");
}

fn scheduler(
    pool: &PgPool,
    runner: &FakeRunner,
    slots: usize,
) -> Arc<ParallelScheduler<FakeRunner>> {
    ParallelScheduler::new(pool.clone(), runner.clone(), config(slots), |_| {
        Some(COMMIT.to_owned())
    })
    .unwrap()
}

async fn new_run(pool: &PgPool, budget: i64) -> (Uuid, Uuid) {
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

/// Task manual READY dengan scope dan dependency tertentu (butuh 150 token budget).
async fn ready(
    pool: &PgPool,
    id: &str,
    ownership: (Uuid, Uuid),
    priority: i32,
    paths: &[&str],
    deps: &[&str],
) {
    TaskRepository::new(pool.clone())
        .create(&TaskContract {
            id: text(id),
            project_id: text(ownership.0.to_string()),
            project_run_id: text(ownership.1.to_string()),
            title: text("task"),
            role: text("worker"),
            objective: text("ship"),
            depends_on: deps.iter().map(|dep| text(*dep)).collect(),
            allowed_paths: paths
                .iter()
                .map(|path| AllowedPath::parse(*path).unwrap())
                .collect(),
            context_refs: vec![],
            acceptance_criteria: vec![text("ok")],
            verification_commands: vec![text("cargo test")],
            limits: TaskLimits {
                max_input_tokens: limit(100),
                max_output_tokens: limit(50),
                max_tool_calls: limit(5),
                max_attempts: MaxAttempts::new(2).unwrap(),
                timeout_seconds: limit(60),
            },
        })
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET status='READY',priority=$2 WHERE id=$1")
        .bind(id)
        .bind(priority)
        .execute(pool)
        .await
        .unwrap();
}

async fn status(pool: &PgPool, task: &str) -> String {
    sqlx::query_scalar("SELECT status FROM tasks WHERE id=$1")
        .bind(task)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn attempts(pool: &PgPool, task: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM agent_runs WHERE task_id=$1")
        .bind(task)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn fills_free_slots_up_to_the_cap_and_refills_idle_ones(pool: PgPool) {
    let own = new_run(&pool, 1_000_000).await;
    for index in 0..6 {
        ready(
            &pool,
            &format!("t{index}"),
            own,
            10 - index,
            &[&format!("src/t{index}/**")],
            &[],
        )
        .await;
    }
    let runner = FakeRunner::new(pool.clone());
    runner.gate(&["t0", "t1", "t2", "t3", "t4", "t5"]);
    let sched = scheduler(&pool, &runner, 3);

    assert_eq!(sched.step().await.unwrap().started, ["t0", "t1", "t2"]);
    assert_eq!(sched.busy_slots(), 3);
    // Runner berjalan di task terpisah; tunggu ketiganya benar-benar aktif sebelum ada yang dilepas.
    eventually("tiga runner aktif", || async {
        runner.started().len() == 3
    })
    .await;
    // Slot penuh: backpressure, tidak ada claim tambahan walau masih ada task siap.
    assert!(sched.step().await.unwrap().started.is_empty());
    assert_eq!(attempts(&pool, "t3").await, 0);

    runner.release("t0");
    eventually("slot kosong", || async { sched.free_slots() == 1 }).await;
    assert_eq!(sched.step().await.unwrap().started, ["t3"]);
    assert_eq!(
        runner.peak.load(Ordering::SeqCst),
        3,
        "tidak pernah lebih dari jumlah slot"
    );
    runner.release_all();
    sched.wait_idle().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn only_dependency_ready_tasks_start(pool: PgPool) {
    let own = new_run(&pool, 1_000_000).await;
    ready(&pool, "a", own, 3, &["src/a/**"], &[]).await;
    ready(&pool, "b", own, 2, &["src/b/**"], &["a"]).await;
    ready(&pool, "c", own, 1, &["src/c/**"], &[]).await;
    let runner = FakeRunner::new(pool.clone());
    runner.gate(&["a"]);
    let sched = scheduler(&pool, &runner, 4);

    assert_eq!(sched.step().await.unwrap().started, ["a", "c"]);
    assert_eq!(attempts(&pool, "b").await, 0, "b menunggu a");
    runner.release("a");
    eventually("a selesai", || async { status(&pool, "a").await == "DONE" }).await;
    assert_eq!(sched.step().await.unwrap().started, ["b"]);
    sched.wait_idle().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn overlapping_scopes_never_run_together(pool: PgPool) {
    let own = new_run(&pool, 1_000_000).await;
    ready(&pool, "hi", own, 9, &["src/**"], &[]).await;
    ready(&pool, "lo", own, 5, &["src/api/x.rs"], &[]).await;
    ready(&pool, "docs", own, 1, &["docs/**"], &[]).await;
    let runner = FakeRunner::new(pool.clone());
    runner.gate(&["hi"]);
    let sched = scheduler(&pool, &runner, 4);

    let report = sched.step().await.unwrap();
    assert_eq!(report.started, ["hi", "docs"]);
    assert_eq!(
        report.skipped,
        [(
            "lo".to_owned(),
            Skip::Lease {
                holder_task_id: "hi".to_owned()
            }
        )]
    );
    // Tidak ada perubahan state untuk task yang tertahan lease: tetap READY tanpa attempt (tanpa flapping).
    assert_eq!(
        (status(&pool, "lo").await, attempts(&pool, "lo").await),
        ("READY".to_owned(), 0)
    );

    runner.release("hi");
    eventually("lease hi lepas", || async {
        LeaseStore::new(pool.clone())
            .active(own.1)
            .await
            .unwrap()
            .is_empty()
    })
    .await;
    assert_eq!(sched.step().await.unwrap().started, ["lo"]);
    sched.wait_idle().await;
    assert_eq!(runner.overlaps.load(Ordering::SeqCst), 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn two_scheduler_instances_never_share_or_overlap(pool: PgPool) {
    let own = new_run(&pool, 1_000_000).await;
    // Enam scope terpisah + dua task beririsan berprioritas rendah (t4 dengan t0, t5 dengan semuanya).
    let specs: [(&str, i32, &str); 8] = [
        ("t0", 9, "src/a/**"),
        ("t1", 8, "src/b/**"),
        ("t2", 7, "src/c/**"),
        ("t3", 6, "src/d/**"),
        ("t4", 3, "src/a/x.rs"),
        ("t5", 2, "src/**"),
        ("t6", 5, "docs/**"),
        ("t7", 4, "tests/**"),
    ];
    for (id, priority, path) in specs {
        ready(&pool, id, own, priority, &[path], &[]).await;
    }
    let runner = FakeRunner::new(pool.clone());
    runner.gate(&specs.map(|spec| spec.0));
    let (first, second) = (scheduler(&pool, &runner, 2), scheduler(&pool, &runner, 2));

    let (left, right) = tokio::join!(first.step(), second.step());
    let started: Vec<String> = left
        .unwrap()
        .started
        .into_iter()
        .chain(right.unwrap().started)
        .collect();
    let unique: HashSet<_> = started.iter().collect();
    assert_eq!(
        (started.len(), unique.len()),
        (4, 4),
        "empat slot terisi, tak ada task ganda: {started:?}"
    );
    let scopes: HashMap<_, _> = specs
        .iter()
        .map(|(id, _, path)| (*id, PathScope::parse(path).unwrap()))
        .collect();
    for a in &started {
        for b in &started {
            assert!(
                a == b || !scopes[a.as_str()].overlaps(&scopes[b.as_str()]),
                "{a} dan {b} beririsan"
            );
        }
    }
    let duplicated: i64 = sqlx::query_scalar("SELECT count(*) FROM (SELECT task_id FROM agent_runs GROUP BY task_id HAVING count(*) > 1) d")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(duplicated, 0);

    // Lepas semua dan biarkan keduanya menyelesaikan sisa; setiap task dimulai tepat sekali dan tanpa tumpang tindih.
    runner.release_all();
    for _ in 0..40 {
        first.wait_idle().await;
        second.wait_idle().await;
        let (a, b) = tokio::join!(first.step(), second.step());
        if a.unwrap().started.is_empty() && b.unwrap().started.is_empty() {
            break;
        }
    }
    first.wait_idle().await;
    second.wait_idle().await;
    let mut all = runner.started();
    all.sort();
    assert_eq!(all, ["t0", "t1", "t2", "t3", "t4", "t5", "t6", "t7"]);
    assert_eq!(runner.overlaps.load(Ordering::SeqCst), 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn run_at_its_budget_stop_is_skipped_but_other_runs_proceed(pool: PgPool) {
    // Run 1: batas kerja 850 dan sudah terpakai 850 -> level Stop.
    let spent = new_run(&pool, 1_000).await;
    let fine = new_run(&pool, 1_000_000).await;
    ready(&pool, "history", spent, 0, &["src/h/**"], &[]).await;
    let tasks = TaskRepository::new(pool.clone());
    let attempt = AgentAttempt {
        id: Uuid::new_v4(),
        task_id: text("history"),
        role: text("worker"),
        attempt: 1,
        provider_id: text("p"),
        model_id: text("m"),
        status: text("running"),
    };
    tasks.create_attempt(&attempt).await.unwrap();
    tasks
        .record_usage(
            attempt.id,
            &Usage {
                input_tokens: 850,
                cached_tokens: 0,
                output_tokens: 0,
                tool_calls: 0,
                latency_ms: 1,
                estimated: false,
            },
        )
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET status='DONE' WHERE id='history'")
        .execute(&pool)
        .await
        .unwrap();
    ready(&pool, "over-budget", spent, 9, &["src/o/**"], &[]).await;
    ready(&pool, "fine", fine, 1, &["src/f/**"], &[]).await;
    let runner = FakeRunner::new(pool.clone());
    let sched = scheduler(&pool, &runner, 2);

    let report = sched.step().await.unwrap();
    assert_eq!(report.started, ["fine"]);
    assert_eq!(report.skipped, [("over-budget".to_owned(), Skip::Budget)]);
    sched.wait_idle().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn slots_are_shared_fairly_between_runs(pool: PgPool) {
    let (busy, quiet) = (
        new_run(&pool, 1_000_000).await,
        new_run(&pool, 1_000_000).await,
    );
    for index in 1..=3 {
        ready(
            &pool,
            &format!("h{index}"),
            busy,
            9,
            &[&format!("src/h{index}/**")],
            &[],
        )
        .await;
    }
    ready(&pool, "l1", quiet, 1, &["src/l1/**"], &[]).await;
    let runner = FakeRunner::new(pool.clone());
    runner.gate(&["h1", "h2", "h3", "l1"]);
    let sched = scheduler(&pool, &runner, 2);

    // Run sibuk tidak memonopoli: slot kedua diberikan ke run lain walau prioritasnya lebih rendah.
    assert_eq!(sched.step().await.unwrap().started, ["h1", "l1"]);
    runner.release_all();
    sched.wait_idle().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn cancel_stops_the_slot_and_finalizes_the_task(pool: PgPool) {
    let own = new_run(&pool, 1_000_000).await;
    ready(&pool, "c1", own, 1, &["src/c1/**"], &[]).await;
    let runner = FakeRunner::new(pool.clone());
    runner.gate(&["c1"]);
    let sched = scheduler(&pool, &runner, 2);
    assert_eq!(sched.step().await.unwrap().started, ["c1"]);

    sqlx::query("UPDATE project_runs SET status='CANCELLED' WHERE id=$1")
        .bind(own.1)
        .execute(&pool)
        .await
        .unwrap();
    eventually("runner diminta berhenti", || async {
        runner.stopped() == [("c1".to_owned(), StopReason::Cancel)]
    })
    .await;
    eventually("slot bebas", || async { sched.free_slots() == 2 }).await;
    assert_eq!(status(&pool, "c1").await, "CANCELLED");
    let attempt: (String, Option<String>) =
        sqlx::query_as("SELECT status,error_code FROM agent_runs WHERE task_id='c1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        attempt,
        ("failed".to_owned(), Some("scheduler.cancelled".to_owned()))
    );
    assert!(
        LeaseStore::new(pool.clone())
            .active(own.1)
            .await
            .unwrap()
            .is_empty(),
        "lease dilepas"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn pause_lets_running_work_finish_but_starts_nothing_new(pool: PgPool) {
    let own = new_run(&pool, 1_000_000).await;
    ready(&pool, "p1", own, 3, &["src/p1/**"], &[]).await;
    ready(&pool, "p2", own, 2, &["src/p2/**"], &[]).await;
    let runner = FakeRunner::new(pool.clone());
    runner.gate(&["p1"]);
    let sched = scheduler(&pool, &runner, 1);
    assert_eq!(sched.step().await.unwrap().started, ["p1"]);

    sqlx::query("UPDATE project_runs SET status='PAUSED' WHERE id=$1")
        .bind(own.1)
        .execute(&pool)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await; // beberapa heartbeat
    assert!(
        runner.stopped().is_empty(),
        "pause tidak menginterupsi pekerjaan berjalan"
    );
    runner.release("p1");
    sched.wait_idle().await;
    assert!(
        sched.step().await.unwrap().started.is_empty(),
        "run yang di-pause tidak mengklaim task baru"
    );
    assert_eq!(attempts(&pool, "p2").await, 0);

    sqlx::query("UPDATE project_runs SET status='RUNNING' WHERE id=$1")
        .bind(own.1)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(sched.step().await.unwrap().started, ["p2"]);
    sched.wait_idle().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn shutdown_takes_no_new_work_and_stops_slots_after_the_grace_period(pool: PgPool) {
    let own = new_run(&pool, 1_000_000).await;
    ready(&pool, "s1", own, 3, &["src/s1/**"], &[]).await;
    let runner = FakeRunner::new(pool.clone());
    runner.gate(&["s1"]);
    let sched = scheduler(&pool, &runner, 2);
    assert_eq!(sched.step().await.unwrap().started, ["s1"]);
    ready(&pool, "s2", own, 2, &["src/s2/**"], &[]).await;

    sched.shutdown();
    assert!(
        sched.step().await.unwrap().started.is_empty(),
        "shutdown tidak mengambil task baru"
    );
    let handle = tokio::spawn(sched.clone().run());
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("run berakhir setelah drain")
        .unwrap();

    assert_eq!(runner.stopped(), [("s1".to_owned(), StopReason::Shutdown)]);
    assert_eq!(attempts(&pool, "s2").await, 0);
    assert_eq!(status(&pool, "s2").await, "READY");
    // s1 dibiarkan untuk recovery berikutnya (bukan CANCELLED/FAILED); lease-nya sudah dilepas.
    assert_eq!(status(&pool, "s1").await, "ASSIGNED");
    assert!(
        LeaseStore::new(pool.clone())
            .active(own.1)
            .await
            .unwrap()
            .is_empty()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn lost_claim_stops_the_runner_without_finalizing_anything(pool: PgPool) {
    let own = new_run(&pool, 1_000_000).await;
    ready(&pool, "l1", own, 1, &["src/l1/**"], &[]).await;
    let runner = FakeRunner::new(pool.clone());
    runner.gate(&["l1"]);
    let sched = scheduler(&pool, &runner, 1);
    assert_eq!(sched.step().await.unwrap().started, ["l1"]);

    // Scheduler lain mengambil alih attempt ini (owner berubah).
    sqlx::query("UPDATE agent_runs SET dispatch_owner=gen_random_uuid() WHERE task_id='l1'")
        .execute(&pool)
        .await
        .unwrap();
    eventually("runner berhenti karena claim hilang", || async {
        runner.stopped() == [("l1".to_owned(), StopReason::Lost)]
    })
    .await;
    sched.wait_idle().await;
    assert_eq!(
        status(&pool, "l1").await,
        "ASSIGNED",
        "pemilik baru yang memutuskan nasib task"
    );
    assert!(
        LeaseStore::new(pool.clone())
            .active(own.1)
            .await
            .unwrap()
            .is_empty()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn recovery_requeues_stale_claims_and_frees_their_leases(pool: PgPool) {
    let own = new_run(&pool, 1_000_000).await;
    ready(&pool, "r1", own, 1, &["src/r1/**"], &[]).await;
    // Instance yang mati: sempat klaim dan memegang lease, lalu heartbeat berhenti.
    let dead = Uuid::new_v4();
    LeaseStore::new(pool.clone())
        .acquire(own.1, "r1", dead, &["src/r1/**"], 600)
        .await
        .unwrap();
    let claimed = SchedulerStore::new(pool.clone())
        .claim_next(
            &ClaimRequest {
                owner: dead,
                provider_id: "p",
                model_id: "m",
                retention_seconds: 60,
                only_task: None,
            },
            |_| Some(COMMIT.to_owned()),
        )
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE agent_runs SET heartbeat_at=now()-interval '1 hour' WHERE id=$1")
        .bind(claimed.attempt.id)
        .execute(&pool)
        .await
        .unwrap();

    let runner = FakeRunner::new(pool.clone());
    let sched = scheduler(&pool, &runner, 1);
    // Lease milik instance mati menahan task selama belum dipulihkan.
    assert_eq!(sched.recover().await.unwrap(), 1);
    assert_eq!(status(&pool, "r1").await, "READY");
    assert!(
        LeaseStore::new(pool.clone())
            .active(own.1)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(sched.step().await.unwrap().started, ["r1"]);
    sched.wait_idle().await;
    assert_eq!(attempts(&pool, "r1").await, 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn invalid_configuration_is_rejected(pool: PgPool) {
    let runner = FakeRunner::new(pool.clone());
    let build = |mutate: fn(&mut ParallelConfig)| {
        let mut cfg = config(2);
        mutate(&mut cfg);
        ParallelScheduler::new(pool.clone(), runner.clone(), cfg, |_| None).err()
    };
    assert!(matches!(
        build(|c| c.slots = 0),
        Some(SchedulerError::Config("slots"))
    ));
    assert!(matches!(
        build(|c| c.slots = 5),
        Some(SchedulerError::Config("slots"))
    ));
    assert!(matches!(
        build(|c| c.heartbeat_interval = Duration::ZERO),
        Some(SchedulerError::Config("intervals"))
    ));
    assert!(matches!(
        build(|c| c.idle_max = Duration::from_millis(1)),
        Some(SchedulerError::Config("intervals"))
    ));
    assert!(matches!(
        build(|c| c.lease_ttl_seconds = 0),
        Some(SchedulerError::Config("lease_ttl_seconds"))
    ));
    assert!(matches!(
        build(|c| c.stale_after_seconds = 0),
        Some(SchedulerError::Config("stale_after_seconds"))
    ));
    assert!(matches!(
        build(|c| c.candidate_window = 0),
        Some(SchedulerError::Config("candidate_window"))
    ));
    assert!(build(|_| {}).is_none());
}

/// Regresi (benchmark M5-010 dengan model nyata): run tidak pernah berpindah ke DONE walau semua task-nya DONE.
#[sqlx::test(migrations = "./migrations")]
async fn run_is_completed_only_when_every_task_is_done(pool: PgPool) {
    let finished = new_run(&pool, 1_000_000).await;
    ready(&pool, "f1", finished, 1, &["src/f1/**"], &[]).await;
    ready(&pool, "f2", finished, 1, &["src/f2/**"], &[]).await;
    let partial = new_run(&pool, 1_000_000).await;
    ready(&pool, "p1", partial, 1, &["src/p1/**"], &[]).await;
    ready(&pool, "p2", partial, 1, &["src/p2/**"], &[]).await;
    let paused = new_run(&pool, 1_000_000).await;
    ready(&pool, "x1", paused, 1, &["src/x1/**"], &[]).await;
    let empty = new_run(&pool, 1_000_000).await;
    sqlx::query("UPDATE tasks SET status='DONE' WHERE id IN ('f1','f2','p1','x1')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE project_runs SET status='PAUSED' WHERE id=$1")
        .bind(paused.1)
        .execute(&pool)
        .await
        .unwrap();

    let sched = scheduler(&pool, &FakeRunner::new(pool.clone()), 1);
    assert_eq!(sched.complete_finished_runs().await.unwrap(), 1);
    assert_eq!(sched.complete_finished_runs().await.unwrap(), 0);
    let run_status = |run: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT status FROM project_runs WHERE id=$1")
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(run_status(finished.1).await, "DONE");
    assert_eq!(run_status(partial.1).await, "RUNNING");
    assert_eq!(run_status(paused.1).await, "PAUSED");
    assert_eq!(run_status(empty.1).await, "RUNNING");
}
