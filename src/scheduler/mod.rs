use std::path::Path;

use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::{
    orchestrator::repository_head,
    store::{
        event::RuntimeAttempt,
        provider::{ProviderRepository, StoreError as ProviderStoreError},
        task::{StoreError, TaskRepository},
    },
};

// ponytail: reservasi plan memakai batas token saat approval; rekonsiliasi usage menunggu M4-003.
pub struct SequentialScheduler {
    pool: PgPool,
    tasks: TaskRepository,
    providers: ProviderRepository,
    model_id: String,
    retention_seconds: i64,
}

// Menerjemahkan error lookup model (tipe milik provider) ke tipe task tanpa membuang penyebabnya.
// ValidationError provider dan task adalah tipe berbeda, jadi baris korup dicatat di log lalu
// dilaporkan sebagai InvalidId; NotFound ditangani pemanggil, Conflict hanya muncul dari INSERT.
fn model_lookup_error(error: ProviderStoreError) -> StoreError {
    match error {
        ProviderStoreError::Database(error) => StoreError::Database(error),
        ProviderStoreError::InvalidRowJson(error) => StoreError::InvalidJson(error),
        other => {
            tracing::error!(cause = %other, "lookup model scheduler gagal");
            StoreError::InvalidId("model_id")
        }
    }
}

impl SequentialScheduler {
    pub fn new(pool: PgPool, model_id: String, retention_seconds: i64) -> Self {
        Self {
            tasks: TaskRepository::new(pool.clone()),
            providers: ProviderRepository::new(pool.clone()),
            pool,
            model_id,
            retention_seconds,
        }
    }

    pub async fn claim_one(&self) -> Result<Option<RuntimeAttempt>, StoreError> {
        if !(1..=crate::domain::task::MAX_SAFE_INTEGER).contains(&self.retention_seconds) {
            return Err(StoreError::InvalidId("retention_seconds"));
        }
        let mut tx = self.pool.begin().await.map_err(StoreError::Database)?;
        // ponytail: satu advisory lock membatasi seluruh run ke satu worker; M4 menggantinya dengan slot paralel.
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(311, 11)")
            .fetch_one(&mut *tx)
            .await
            .map_err(StoreError::Database)?;
        if !locked {
            return Ok(None);
        }
        // Kunci run dahulu: approval, pause, cancel, dan claim lain memakai urutan kunci yang sama.
        let candidate = sqlx::query(
            "SELECT t.id,t.role,t.version,t.max_input_tokens,t.max_output_tokens,t.plan_id,
                    pr.id AS run_id,pr.reserved_tokens,pr.token_budget,p.repository_path
             FROM project_runs pr JOIN tasks t ON t.project_run_id=pr.id
             JOIN projects p ON p.id=pr.project_id
             WHERE pr.status='RUNNING' AND t.status='READY'
               AND (t.plan_id IS NULL OR EXISTS (SELECT 1 FROM plans plan
                   WHERE plan.id=t.plan_id AND plan.project_run_id=pr.id AND plan.status='APPROVED'))
               AND (SELECT count(*) FROM agent_runs ar WHERE ar.task_id=t.id) < t.max_attempts
               AND NOT EXISTS (SELECT 1 FROM task_dependencies d JOIN tasks required ON required.id=d.dependency_id
                               WHERE d.task_id=t.id AND required.status <> 'DONE')
               AND NOT EXISTS (SELECT 1 FROM tasks active
                               WHERE active.status IN ('ASSIGNED','RUNNING','SELF_CHECK','REVIEW','VERIFY','INTEGRATE'))
               AND ((t.plan_id IS NOT NULL AND pr.reserved_tokens >= t.max_input_tokens + t.max_output_tokens)
                    OR (t.plan_id IS NULL AND pr.reserved_tokens + t.max_input_tokens + t.max_output_tokens <= pr.token_budget))
             ORDER BY t.priority DESC,t.created_at,t.id
             FOR UPDATE OF pr SKIP LOCKED LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await.map_err(StoreError::Database)?;
        let Some(row) = candidate else {
            return Ok(None);
        };
        // Model dicari setelah ada kandidat: scheduler idle tidak butuh model. Model belum
        // terdaftar (mis. provider baru dibuat lewat UI) bukan error; task menunggu, tx di-rollback.
        let model = match self.providers.get_model(&self.model_id).await {
            Ok(model) => model,
            Err(ProviderStoreError::NotFound) => {
                tracing::warn!(
                    model_id = %self.model_id,
                    "scheduler menunggu: model default belum terdaftar"
                );
                return Ok(None);
            }
            Err(error) => return Err(model_lookup_error(error)),
        };
        let task_id: String = row.get("id");
        let task = sqlx::query("SELECT status,version FROM tasks WHERE id=$1 FOR UPDATE")
            .bind(&task_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(StoreError::Database)?;
        if task.get::<&str, _>("status") != "READY"
            || task.get::<i64, _>("version") != row.get::<i64, _>("version")
        {
            return Ok(None);
        }
        // Snapshot kandidat dapat usang ketika scheduler lain menunggu kunci run.
        let eligible: bool = sqlx::query_scalar(
            "SELECT pr.status='RUNNING'
               AND (t.plan_id IS NULL OR EXISTS (SELECT 1 FROM plans plan
                   WHERE plan.id=t.plan_id AND plan.project_run_id=pr.id AND plan.status='APPROVED'))
               AND (SELECT count(*) FROM agent_runs ar WHERE ar.task_id=t.id) < t.max_attempts
               AND NOT EXISTS (SELECT 1 FROM task_dependencies d JOIN tasks required ON required.id=d.dependency_id
                               WHERE d.task_id=$1 AND required.status <> 'DONE')
               AND NOT EXISTS (SELECT 1 FROM tasks active
                               WHERE active.status IN ('ASSIGNED','RUNNING','SELF_CHECK','REVIEW','VERIFY','INTEGRATE'))
             FROM project_runs pr JOIN tasks t ON t.project_run_id=pr.id WHERE pr.id=$2 AND t.id=$1",
        )
        .bind(&task_id)
        .bind(row.get::<Uuid, _>("run_id"))
        .fetch_one(&mut *tx)
        .await
        .map_err(StoreError::Database)?;
        if !eligible {
            return Ok(None);
        }
        let needed = row
            .get::<i64, _>("max_input_tokens")
            .checked_add(row.get::<i64, _>("max_output_tokens"))
            .ok_or(StoreError::InvalidId("budget"))?;
        let reserved: i64 = row.get("reserved_tokens");
        let budget: i64 = row.get("token_budget");
        // Approved plans already reserved every task at approval. Manual tasks reserve at claim.
        if row.get::<Option<String>, _>("plan_id").is_some() {
            if reserved < needed || reserved > budget {
                return Ok(None);
            }
        } else {
            let Some(total) = reserved.checked_add(needed) else {
                return Ok(None);
            };
            if total > budget {
                return Ok(None);
            }
            sqlx::query("UPDATE project_runs SET reserved_tokens=$2 WHERE id=$1")
                .bind(row.get::<Uuid, _>("run_id"))
                .bind(total)
                .execute(&mut *tx)
                .await
                .map_err(StoreError::Database)?;
        }
        let base_commit = repository_head(Path::new(row.get::<&str, _>("repository_path")))
            .ok_or(StoreError::InvalidId("base_commit"))?;
        let id = Uuid::new_v4();
        let branch = format!("noctis-{task_id}-{}", id.simple());
        let attempt: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(attempt),0)+1 FROM agent_runs WHERE task_id=$1",
        )
        .bind(&task_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(StoreError::Database)?;
        sqlx::query(
            "UPDATE tasks SET status='ASSIGNED',version=version+1,updated_at=now() WHERE id=$1",
        )
        .bind(&task_id)
        .execute(&mut *tx)
        .await
        .map_err(StoreError::Database)?;
        sqlx::query("INSERT INTO events (project_run_id,task_id,actor_type,event_type,from_status,to_status,payload) VALUES ($1,$2,'system','status_transition','READY','ASSIGNED','{}')")
            .bind(row.get::<Uuid, _>("run_id")).bind(&task_id)
            .execute(&mut *tx).await.map_err(StoreError::Database)?;
        sqlx::query("INSERT INTO agent_runs (id,task_id,role,provider_id,model_id,attempt,status,branch,base_commit,heartbeat_at,retain_until) VALUES ($1,$2,$3,$4,$5,$6,'assigned',$7,$8,now(),now()+($9 * interval '1 second'))")
            .bind(id).bind(&task_id).bind(row.get::<&str, _>("role"))
            .bind(model.provider_id.as_str()).bind(model.id.as_str()).bind(attempt)
            .bind(branch).bind(base_commit).bind(self.retention_seconds)
            .execute(&mut *tx).await.map_err(StoreError::Database)?;
        tx.commit().await.map_err(StoreError::Database)?;
        self.tasks.get_attempt(id).await.map(Some)
    }
}
