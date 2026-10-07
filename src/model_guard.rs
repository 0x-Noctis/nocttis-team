//! Implementasi produksi `CallGuard` untuk `ModelRouter` (M5-005), berbasis PostgreSQL.
//!
//! Gagal TERTUTUP: bila database tidak terjangkau atau data tidak terbaca, retry/fallback dianggap tidak aman dan
//! budget dianggap habis. Lebih baik berhenti daripada mengulang tanpa kepastian.

use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    domain::budget::BudgetLevel,
    model::{ModelRequest, router::CallGuard},
    store::budget::BudgetStore,
};

#[derive(Clone)]
pub struct PgCallGuard {
    pool: PgPool,
}

impl PgCallGuard {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl CallGuard for PgCallGuard {
    /// Aman bila attempt tidak punya reservasi tool yang masih `in_progress` (efek samping yang hasilnya belum pasti).
    async fn side_effects_safe(&self, agent_run_id: &str) -> bool {
        let Ok(run) = Uuid::parse_str(agent_run_id) else {
            return false;
        };
        sqlx::query_scalar::<_, bool>(
            "SELECT NOT EXISTS (SELECT 1 FROM tool_call_reservations WHERE agent_run_id=$1 AND status='in_progress')",
        )
        .bind(run)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false)
    }

    /// Budget run (setelah cadangan) belum mencapai level Stop.
    async fn budget_allows(&self, request: &ModelRequest) -> bool {
        let Ok(attempt) = Uuid::parse_str(&request.agent_run_id) else {
            return false;
        };
        let run: Option<Uuid> = sqlx::query_scalar(
            "SELECT t.project_run_id::uuid FROM agent_runs a JOIN tasks t ON t.id=a.task_id WHERE a.id=$1",
        )
        .bind(attempt)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();
        let Some(run) = run else {
            return false;
        };
        BudgetStore::new(self.pool.clone())
            .run_view(run)
            .await
            .is_ok_and(|view| view.level != BudgetLevel::Stop)
    }
}
