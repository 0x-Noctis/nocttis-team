// M5-005: matriks retry/fallback dengan jam palsu (tidak ada tidur sungguhan) dan model terskrip.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use ai_team::model::{
    FinishReason, Message, MessageRole, ModelError, ModelErrorKind, ModelLimits, ModelRequest,
    ModelResponse, ToolDefinition, Usage,
    retry::{HARD_MAX_ATTEMPTS, HARD_MAX_DELAY, RetryPolicy, Sleeper},
    router::{
        Attempt, CallGuard, Candidate, CompletionModel, ModelProfile, ModelRouter, Step, StopReason,
    },
};
use serde_json::json;

type Log = Arc<Mutex<Vec<String>>>;

struct Scripted {
    id: &'static str,
    results: VecDeque<Result<(), ModelErrorKind>>,
    calls: Log,
}

impl CompletionModel for Scripted {
    async fn complete(&mut self, _: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.calls.lock().unwrap().push(self.id.to_owned());
        match self
            .results
            .pop_front()
            .unwrap_or(Err(ModelErrorKind::ProviderUnavailable))
        {
            Ok(()) => Ok(ModelResponse {
                content: Some(self.id.to_owned()),
                tool_calls: Vec::new(),
                finish_reason: FinishReason::Stop,
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cached_tokens: 0,
                    total_tokens: 2,
                    estimated: false,
                },
                latency_ms: 1,
            }),
            Err(kind) => Err(ModelError::new(kind)),
        }
    }
}

#[derive(Clone, Default)]
struct FakeSleeper(Arc<Mutex<Vec<Duration>>>);

impl Sleeper for FakeSleeper {
    async fn sleep(&mut self, duration: Duration) {
        self.0.lock().unwrap().push(duration);
    }
}

#[derive(Clone)]
struct Guard {
    safe: Arc<Mutex<bool>>,
    budget: Arc<Mutex<bool>>,
}

impl Guard {
    fn new() -> Self {
        Self {
            safe: Arc::new(Mutex::new(true)),
            budget: Arc::new(Mutex::new(true)),
        }
    }
}

impl CallGuard for Guard {
    async fn side_effects_safe(&self, _: &str) -> bool {
        *self.safe.lock().unwrap()
    }
    async fn budget_allows(&self, _: &ModelRequest) -> bool {
        *self.budget.lock().unwrap()
    }
}

fn profile(id: &str, class: &str, tools: bool, window: u64) -> ModelProfile {
    ModelProfile {
        id: id.into(),
        class: class.into(),
        tools_verified: tools,
        context_window: window,
    }
}

fn request(tools: bool) -> ModelRequest {
    ModelRequest {
        project_id: "p".into(),
        task_id: "t".into(),
        agent_run_id: "r".into(),
        model_class: "coding".into(),
        messages: vec![Message {
            role: MessageRole::User,
            content: "hi".into(),
            tool_call_id: None,
        }],
        tools: if tools {
            vec![ToolDefinition {
                name: "x".into(),
                description: "x".into(),
                input_schema: json!({"type": "object"}),
            }]
        } else {
            Vec::new()
        },
        limits: ModelLimits {
            max_input_tokens: 1_000,
            max_output_tokens: 100,
        },
    }
}

fn policy(attempts: u32) -> RetryPolicy {
    RetryPolicy::new(
        attempts,
        Duration::from_millis(500),
        Duration::from_secs(8),
        Duration::from_secs(60),
    )
}

/// (id, class, tools_verified, context_window, skrip hasil).
type ModelSpec = (
    &'static str,
    &'static str,
    bool,
    u64,
    Vec<Result<(), ModelErrorKind>>,
);

struct Fixture {
    router: ModelRouter<Scripted, FakeSleeper, Guard>,
    calls: Log,
    sleeps: Arc<Mutex<Vec<Duration>>>,
    guard: Guard,
}

/// `models`: (id, class, tools_verified, context_window, skrip hasil).
fn fixture(policy: RetryPolicy, models: Vec<ModelSpec>) -> Fixture {
    let calls = Log::default();
    let sleeper = FakeSleeper::default();
    let sleeps = sleeper.0.clone();
    let guard = Guard::new();
    let candidates = models
        .into_iter()
        .map(|(id, class, tools, window, results)| Candidate {
            profile: profile(id, class, tools, window),
            model: Scripted {
                id,
                results: results.into(),
                calls: calls.clone(),
            },
        })
        .collect();
    Fixture {
        router: ModelRouter::new(candidates, policy, sleeper, guard.clone()),
        calls,
        sleeps,
        guard,
    }
}

fn calls(fixture: &Fixture) -> Vec<String> {
    fixture.calls.lock().unwrap().clone()
}

#[tokio::test]
async fn permanent_errors_are_never_retried_and_never_fall_back() {
    for kind in [
        ModelErrorKind::AuthenticationFailed,
        ModelErrorKind::InvalidResponse,
        ModelErrorKind::ContextTooLarge,
        ModelErrorKind::BudgetExceeded,
    ] {
        let mut f = fixture(
            policy(3),
            vec![
                ("primary", "coding", true, 10_000, vec![Err(kind), Ok(())]),
                ("backup", "coding", true, 10_000, vec![Ok(())]),
            ],
        );
        let error = f.router.complete(&request(true)).await.unwrap_err();
        assert_eq!(error.kind(), kind);
        assert_eq!(
            calls(&f),
            ["primary"],
            "{kind:?}: tepat satu panggilan, tanpa fallback"
        );
        assert!(f.sleeps.lock().unwrap().is_empty(), "{kind:?}");
        assert_eq!(
            f.router.take_report().last().unwrap().step,
            Step::Stopped(StopReason::NotRetryable(kind))
        );
    }
}

#[tokio::test]
async fn transient_errors_retry_with_exponential_backoff_then_succeed() {
    for kind in [
        ModelErrorKind::RateLimited,
        ModelErrorKind::Timeout,
        ModelErrorKind::ProviderUnavailable,
    ] {
        let mut f = fixture(
            policy(4),
            vec![(
                "primary",
                "coding",
                true,
                10_000,
                vec![Err(kind), Err(kind), Ok(())],
            )],
        );
        let response = f.router.complete(&request(true)).await.unwrap();
        assert_eq!(response.content.as_deref(), Some("primary"));
        assert_eq!(calls(&f).len(), 3, "{kind:?}");
        assert_eq!(
            *f.sleeps.lock().unwrap(),
            [Duration::from_millis(500), Duration::from_secs(1)],
            "{kind:?}: 500ms lalu 1s"
        );
        let steps: Vec<_> = f.router.take_report().into_iter().map(|a| a.step).collect();
        assert_eq!(
            steps,
            [Step::Retried(kind), Step::Retried(kind), Step::Succeeded]
        );
    }
}

#[test]
fn backoff_is_exponential_capped_and_hard_limited() {
    let p = RetryPolicy::new(
        10,
        Duration::from_millis(500),
        Duration::from_secs(4),
        Duration::from_secs(60),
    );
    let delays: Vec<_> = (1..=6).map(|n| p.delay_before(n)).collect();
    assert_eq!(
        delays,
        [500, 1_000, 2_000, 4_000, 4_000, 4_000].map(Duration::from_millis)
    );
    // Nomor retry sangat besar tidak overflow.
    assert_eq!(p.delay_before(u32::MAX), Duration::from_secs(4));
    // Konfigurasi liar dipotong ke batas keras.
    let wild = RetryPolicy::new(
        1_000,
        Duration::from_secs(3_600),
        Duration::from_secs(3_600),
        Duration::from_secs(99_999),
    );
    assert_eq!(wild.max_attempts(), HARD_MAX_ATTEMPTS);
    assert!(wild.delay_before(1) <= HARD_MAX_DELAY);
    assert!(wild.max_total_delay() <= HARD_MAX_DELAY * 2);
    assert_eq!(
        RetryPolicy::new(0, Duration::ZERO, Duration::ZERO, Duration::ZERO).max_attempts(),
        1
    );
}

#[tokio::test]
async fn total_sleep_per_model_is_bounded_even_when_attempts_remain() {
    // Percobaan masih tersisa (6) tetapi total tidur dibatasi 2 detik: 0.5 + 1.0 = 1.5 s, jeda 2.0 s berikutnya tidak muat.
    let bounded = RetryPolicy::new(
        6,
        Duration::from_millis(500),
        Duration::from_secs(8),
        Duration::from_secs(2),
    );
    let mut f = fixture(
        bounded,
        vec![(
            "primary",
            "coding",
            true,
            10_000,
            vec![Err(ModelErrorKind::Timeout); 6],
        )],
    );
    let error = f.router.complete(&request(true)).await.unwrap_err();
    assert_eq!(error.kind(), ModelErrorKind::Timeout);
    assert_eq!(calls(&f).len(), 3);
    let total: Duration = f.sleeps.lock().unwrap().iter().sum();
    assert!(total <= Duration::from_secs(2), "{total:?}");
}

#[tokio::test]
async fn exhausted_primary_falls_back_to_a_compatible_model() {
    let mut f = fixture(
        policy(3),
        vec![
            (
                "primary",
                "coding",
                true,
                10_000,
                vec![Err(ModelErrorKind::RateLimited); 3],
            ),
            ("backup", "coding", true, 10_000, vec![Ok(())]),
        ],
    );
    let response = f.router.complete(&request(true)).await.unwrap();
    assert_eq!(response.content.as_deref(), Some("backup"));
    assert_eq!(calls(&f), ["primary", "primary", "primary", "backup"]);
    let report = f.router.take_report();
    assert_eq!(
        report
            .iter()
            .map(|a| (a.model_id.as_str(), a.step.clone()))
            .collect::<Vec<_>>(),
        [
            ("primary", Step::Retried(ModelErrorKind::RateLimited)),
            ("primary", Step::Retried(ModelErrorKind::RateLimited)),
            ("primary", Step::FellBack(ModelErrorKind::RateLimited)),
            ("backup", Step::Succeeded),
        ]
    );
}

#[tokio::test]
async fn fallback_requires_same_class_verified_tools_and_enough_context() {
    let transient = || vec![Err(ModelErrorKind::ProviderUnavailable); 3];
    let mut f = fixture(
        policy(3),
        vec![
            ("primary", "coding", true, 10_000, transient()),
            ("other-class", "reasoning", true, 10_000, vec![Ok(())]),
            ("no-tools", "coding", false, 10_000, vec![Ok(())]),
            ("small", "coding", true, 500, vec![Ok(())]),
            ("good", "coding", true, 10_000, vec![Ok(())]),
        ],
    );
    let response = f.router.complete(&request(true)).await.unwrap();
    assert_eq!(response.content.as_deref(), Some("good"));
    assert!(
        !calls(&f)
            .iter()
            .any(|id| ["other-class", "no-tools", "small"].contains(&id.as_str()))
    );

    // Tanpa tools, model yang tools-nya belum terverifikasi boleh jadi fallback; class tetap harus sama.
    let mut f = fixture(
        policy(3),
        vec![
            ("primary", "coding", true, 10_000, transient()),
            ("other-class", "reasoning", true, 10_000, vec![Ok(())]),
            ("no-tools", "coding", false, 10_000, vec![Ok(())]),
        ],
    );
    let response = f.router.complete(&request(false)).await.unwrap();
    assert_eq!(response.content.as_deref(), Some("no-tools"));
}

#[tokio::test]
async fn all_candidates_exhausted_returns_the_last_error() {
    let mut f = fixture(
        policy(2),
        vec![
            (
                "a",
                "coding",
                true,
                10_000,
                vec![Err(ModelErrorKind::RateLimited); 2],
            ),
            (
                "b",
                "coding",
                true,
                10_000,
                vec![Err(ModelErrorKind::Timeout); 2],
            ),
        ],
    );
    let error = f.router.complete(&request(true)).await.unwrap_err();
    assert_eq!(error.kind(), ModelErrorKind::Timeout);
    assert_eq!(calls(&f), ["a", "a", "b", "b"]);
    assert_eq!(
        f.router
            .take_report()
            .last()
            .map(|a: &Attempt| a.step.clone()),
        Some(Step::Stopped(StopReason::NoCandidate))
    );
}

#[tokio::test]
async fn ambiguous_side_effects_block_both_retry_and_fallback() {
    let mut f = fixture(
        policy(3),
        vec![
            (
                "primary",
                "coding",
                true,
                10_000,
                vec![Err(ModelErrorKind::Timeout), Ok(())],
            ),
            ("backup", "coding", true, 10_000, vec![Ok(())]),
        ],
    );
    *f.guard.safe.lock().unwrap() = false;
    let error = f.router.complete(&request(true)).await.unwrap_err();
    assert_eq!(
        error.kind(),
        ModelErrorKind::Timeout,
        "error asli dikembalikan"
    );
    assert_eq!(calls(&f), ["primary"], "tanpa retry dan tanpa fallback");
    assert!(f.sleeps.lock().unwrap().is_empty());
    assert_eq!(
        f.router.take_report().last().unwrap().step,
        Step::Stopped(StopReason::AmbiguousSideEffect)
    );
}

#[tokio::test]
async fn budget_exhaustion_stops_before_any_call_and_before_any_retry() {
    // Budget sudah habis sejak awal: tidak ada panggilan.
    let mut f = fixture(
        policy(3),
        vec![("primary", "coding", true, 10_000, vec![Ok(())])],
    );
    *f.guard.budget.lock().unwrap() = false;
    let error = f.router.complete(&request(true)).await.unwrap_err();
    assert_eq!(error.kind(), ModelErrorKind::BudgetExceeded);
    assert!(calls(&f).is_empty());

    // Budget habis di antara percobaan: retry tidak dilakukan.
    struct FlipGuard(Arc<Mutex<u32>>);
    impl CallGuard for FlipGuard {
        async fn side_effects_safe(&self, _: &str) -> bool {
            true
        }
        async fn budget_allows(&self, _: &ModelRequest) -> bool {
            let mut checks = self.0.lock().unwrap();
            *checks += 1;
            *checks <= 1
        }
    }
    let log = Log::default();
    let mut router = ModelRouter::new(
        vec![Candidate {
            profile: profile("primary", "coding", true, 10_000),
            model: Scripted {
                id: "primary",
                results: vec![Err(ModelErrorKind::RateLimited), Ok(())].into(),
                calls: log.clone(),
            },
        }],
        policy(3),
        FakeSleeper::default(),
        FlipGuard(Arc::default()),
    );
    let error = router.complete(&request(true)).await.unwrap_err();
    assert_eq!(error.kind(), ModelErrorKind::BudgetExceeded);
    assert_eq!(
        log.lock().unwrap().len(),
        1,
        "tidak ada panggilan ulang setelah budget habis"
    );
}

// ---- Guard produksi (PostgreSQL nyata): gagal tertutup ----
mod pg_guard {
    use super::*;
    use ai_team::model_guard::PgCallGuard;
    use sqlx::PgPool;
    use uuid::Uuid;

    async fn seed(pool: &PgPool) -> (Uuid, Uuid) {
        let (project, run, attempt) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        sqlx::query(
            "INSERT INTO projects (id,name,repository_path) VALUES ($1,'guard','/tmp/guard')",
        )
        .bind(project)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget) VALUES ($1,$2,'g','RUNNING',10000)")
            .bind(run).bind(project).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO providers (id,base_url,api_key_env,request_timeout_seconds) VALUES ('gp','http://127.0.0.1:1','X',1)")
            .execute(pool).await.unwrap();
        sqlx::query("INSERT INTO models (id,provider_id,remote_name,class,context_window,max_output_tokens) VALUES ('gm','gp','m','coding',1000,1000)")
            .execute(pool).await.unwrap();
        sqlx::query("INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,max_attempts) VALUES ('gt',$1,'worker','t','o','RUNNING','[\"a\"]','[\"ok\"]','[\"true\"]',1000,1000,2)")
            .bind(run).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO agent_runs (id,task_id,role,provider_id,model_id,attempt,status,branch,base_commit,heartbeat_at,retain_until) VALUES ($1,'gt','worker','gp','gm',1,'running','b',$2,now(),now()+interval '1 hour')")
            .bind(attempt).bind("0".repeat(40)).execute(pool).await.unwrap();
        (run, attempt)
    }

    fn req(attempt: &str) -> ModelRequest {
        ModelRequest {
            agent_run_id: attempt.to_owned(),
            ..request(true)
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn in_progress_tool_call_makes_retry_unsafe(pool: PgPool) {
        let (_, attempt) = seed(&pool).await;
        let guard = PgCallGuard::new(pool.clone());
        assert!(guard.side_effects_safe(&attempt.to_string()).await);
        sqlx::query("INSERT INTO tool_call_reservations (agent_run_id,call_id,tool_name) VALUES ($1,'call-1','apply_patch')")
            .bind(attempt).execute(&pool).await.unwrap();
        assert!(
            !guard.side_effects_safe(&attempt.to_string()).await,
            "reservasi in_progress = hasil ambigu"
        );
        sqlx::query("UPDATE tool_call_reservations SET status='completed',outcome='succeeded',duration_ms=1,completed_at=now()")
            .execute(&pool).await.unwrap();
        assert!(guard.side_effects_safe(&attempt.to_string()).await);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn run_budget_at_stop_level_blocks_calls(pool: PgPool) {
        let (_, attempt) = seed(&pool).await;
        let guard = PgCallGuard::new(pool.clone());
        assert!(guard.budget_allows(&req(&attempt.to_string())).await);
        // 9.000 token terpakai >= 85% dari 10.000 (cadangan 15%): level Stop.
        sqlx::query("INSERT INTO model_usage (agent_run_id,input_tokens,output_tokens,latency_ms) VALUES ($1,8000,1000,1)")
            .bind(attempt).execute(&pool).await.unwrap();
        assert!(!guard.budget_allows(&req(&attempt.to_string())).await);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unknown_or_malformed_ids_fail_closed(pool: PgPool) {
        seed(&pool).await;
        let guard = PgCallGuard::new(pool);
        for id in ["not-a-uuid", "", &Uuid::new_v4().to_string()] {
            assert!(!guard.budget_allows(&req(id)).await, "budget {id:?}");
        }
        assert!(!guard.side_effects_safe("not-a-uuid").await);
    }
}
