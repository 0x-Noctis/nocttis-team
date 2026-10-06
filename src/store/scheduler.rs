//! Claim task atomik untuk banyak scheduler (M4-001).
//!
//! `claim_next` mengunci baris TASK dengan `FOR UPDATE SKIP LOCKED`, bukan baris run atau advisory lock
//! global, sehingga beberapa scheduler (atau beberapa slot satu scheduler) bisa mengambil task berbeda
//! secara paralel tanpa pernah mengambil task yang sama. Claim = task READY -> ASSIGNED + satu baris
//! `agent_runs` ber-heartbeat, semuanya dalam satu transaksi. Claim yang ditinggalkan (heartbeat berhenti)
//! dideteksi lewat `stale_claims` dan dipulihkan lewat `recover_abandoned`.

use std::path::Path;

use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::store::{
    event::{RecoveryDisposition, RuntimeAttempt},
    task::{Conflict, StoreError, TaskRepository},
};

/// Syarat sebuah task boleh diklaim; dipakai bersama oleh `claim_next` dan `ready_candidates` supaya
/// daftar kandidat selalu sama dengan yang benar-benar bisa diklaim.
const ELIGIBLE: &str = "WHERE pr.status='RUNNING' AND t.status='READY'
               AND (t.plan_id IS NULL OR EXISTS (SELECT 1 FROM plans plan
                   WHERE plan.id=t.plan_id AND plan.project_run_id=pr.id AND plan.status='APPROVED'))
               AND (SELECT count(*) FROM agent_runs ar WHERE ar.task_id=t.id) < t.max_attempts
               AND NOT EXISTS (SELECT 1 FROM task_dependencies d JOIN tasks required ON required.id=d.dependency_id
                               WHERE d.task_id=t.id AND required.status <> 'DONE')
               AND ((t.plan_id IS NOT NULL AND pr.reserved_tokens >= t.max_input_tokens + t.max_output_tokens)
                    OR (t.plan_id IS NULL AND pr.reserved_tokens + t.max_input_tokens + t.max_output_tokens <= pr.token_budget))";

/// Parameter satu kali claim. `owner` identik dengan `agent_runs.dispatch_owner` dan menjadi bukti
/// kepemilikan untuk heartbeat berikutnya.
pub struct ClaimRequest<'a> {
    pub owner: Uuid,
    pub provider_id: &'a str,
    pub model_id: &'a str,
    pub retention_seconds: i64,
    /// Batasi claim ke task ini (setelah scheduler lebih dulu memegang file lease-nya). `None` = kandidat teratas.
    pub only_task: Option<&'a str>,
}

/// Task yang bisa diklaim sekarang beserta scope file-nya.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub task_id: String,
    pub run_id: Uuid,
    pub priority: i32,
    pub allowed_paths: Vec<String>,
}

#[derive(Debug)]
pub struct ClaimedTask {
    pub attempt: RuntimeAttempt,
    pub run_id: Uuid,
    pub repository_path: String,
}

/// Claim yang heartbeat-nya sudah melewati batas stale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaleClaim {
    pub attempt_id: Uuid,
    pub task_id: String,
    pub owner: Option<Uuid>,
}

#[derive(Clone)]
pub struct SchedulerStore {
    pool: PgPool,
    tasks: TaskRepository,
}

impl SchedulerStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            tasks: TaskRepository::new(pool.clone()),
            pool,
        }
    }

    /// Ambil satu task READY yang siap jalan: run RUNNING, plan (bila ada) APPROVED, sisa attempt,
    /// semua dependency DONE, dan budget cukup. Urutan: priority tertinggi, lalu paling lama, lalu id
    /// (stabil). `None` bila tidak ada kandidat atau kandidat berubah di tengah jalan; pemanggil cukup
    /// mencoba lagi pada tick berikutnya.
    ///
    /// `base_commit` dipanggil di dalam transaksi dengan path repository dan harus mengembalikan HEAD
    /// repository itu; dipisah supaya store tidak bergantung pada Git.
    pub async fn claim_next(
        &self,
        request: &ClaimRequest<'_>,
        base_commit: impl FnOnce(&Path) -> Option<String>,
    ) -> Result<Option<ClaimedTask>, StoreError> {
        if !(1..=crate::domain::task::MAX_SAFE_INTEGER).contains(&request.retention_seconds) {
            return Err(StoreError::InvalidId("retention_seconds"));
        }
        let mut tx = self.pool.begin().await.map_err(StoreError::Database)?;
        // Kandidat dikunci per task. Syarat budget ikut di SQL supaya task yang tak muat tidak menutup
        // jalan task berprioritas lebih rendah yang muat (tidak ada starvation oleh kandidat teratas).
        let candidate = sqlx::query(&format!(
            "SELECT t.id,t.role,t.version,t.max_input_tokens,t.max_output_tokens,t.plan_id,
                    pr.id AS run_id,p.repository_path
             FROM tasks t
             JOIN project_runs pr ON pr.id=t.project_run_id
             JOIN projects p ON p.id=pr.project_id
             {ELIGIBLE} AND ($1::text IS NULL OR t.id=$1)
             ORDER BY t.priority DESC,t.created_at,t.id
             FOR UPDATE OF t SKIP LOCKED LIMIT 1"
        ))
        .bind(request.only_task)
        .fetch_optional(&mut *tx)
        .await
        .map_err(StoreError::Database)?;
        let Some(row) = candidate else {
            return Ok(None);
        };
        let task_id: String = row.get("id");
        let run_id: Uuid = row.get("run_id");
        let needed = row
            .get::<i64, _>("max_input_tokens")
            .checked_add(row.get::<i64, _>("max_output_tokens"))
            .ok_or(StoreError::InvalidId("budget"))?;

        // Status run bisa berubah (pause/cancel) sejak snapshot kandidat. Task plan hanya membaca run
        // (FOR SHARE: claim paralel tidak saling blok, pause menunggu kita atau sebaliknya); task manual
        // menaikkan reservasi sehingga butuh FOR UPDATE agar dua claim tidak melewati batas budget.
        let manual = row.get::<Option<String>, _>("plan_id").is_none();
        let lock = if manual { "FOR UPDATE" } else { "FOR SHARE" };
        let run = sqlx::query(&format!(
            "SELECT status,token_budget,reserved_tokens FROM project_runs WHERE id=$1 {lock}"
        ))
        .bind(run_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(StoreError::Database)?;
        if run.get::<&str, _>("status") != "RUNNING" {
            return Ok(None);
        }
        let reserved: i64 = run.get("reserved_tokens");
        if manual {
            let total = reserved.checked_add(needed);
            if total.is_none_or(|total| total > run.get::<i64, _>("token_budget")) {
                return Ok(None);
            }
            sqlx::query("UPDATE project_runs SET reserved_tokens=$2 WHERE id=$1")
                .bind(run_id)
                .bind(total)
                .execute(&mut *tx)
                .await
                .map_err(StoreError::Database)?;
        } else if reserved < needed {
            return Ok(None);
        }

        let repository_path: String = row.get("repository_path");
        let base_commit =
            base_commit(Path::new(&repository_path)).ok_or(StoreError::InvalidId("base_commit"))?;
        let attempt_id = Uuid::new_v4();
        let branch = format!("noctis-{task_id}-{}", attempt_id.simple());
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
            .bind(run_id)
            .bind(&task_id)
            .execute(&mut *tx)
            .await
            .map_err(StoreError::Database)?;
        sqlx::query(
            "INSERT INTO agent_runs
             (id,task_id,role,provider_id,model_id,attempt,status,branch,base_commit,heartbeat_at,retain_until,dispatch_owner,dispatch_claimed_at)
             VALUES ($1,$2,$3,$4,$5,$6,'assigned',$7,$8,now(),now()+($9 * interval '1 second'),$10,now())",
        )
        .bind(attempt_id)
        .bind(&task_id)
        .bind(row.get::<&str, _>("role"))
        .bind(request.provider_id)
        .bind(request.model_id)
        .bind(attempt)
        .bind(branch)
        .bind(base_commit)
        .bind(request.retention_seconds)
        .bind(request.owner)
        .execute(&mut *tx)
        .await
        .map_err(StoreError::Database)?;
        tx.commit().await.map_err(StoreError::Database)?;
        Ok(Some(ClaimedTask {
            attempt: self.tasks.get_attempt(attempt_id).await?,
            run_id,
            repository_path,
        }))
    }

    /// Task yang saat ini memenuhi syarat klaim, urut seperti `claim_next` (priority, umur, id). Hanya membaca
    /// dan tidak mengunci: hasilnya petunjuk bagi scheduler (mis. untuk memegang file lease lebih dulu);
    /// keputusan akhir tetap di `claim_next`, yang bisa saja mengembalikan `None` bila task sudah diambil.
    pub async fn ready_candidates(&self, limit: i64) -> Result<Vec<Candidate>, StoreError> {
        if !(1..=200).contains(&limit) {
            return Err(StoreError::InvalidId("limit"));
        }
        let rows = sqlx::query(&format!(
            "SELECT t.id,t.priority,t.allowed_paths,pr.id AS run_id
             FROM tasks t JOIN project_runs pr ON pr.id=t.project_run_id
             {ELIGIBLE}
             ORDER BY t.priority DESC,t.created_at,t.id LIMIT $1"
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        rows.iter()
            .map(|row| {
                let paths: Vec<String> = serde_json::from_value(row.get("allowed_paths"))
                    .map_err(StoreError::InvalidJson)?;
                Ok(Candidate {
                    task_id: row.get("id"),
                    run_id: row.get("run_id"),
                    priority: row.get("priority"),
                    allowed_paths: paths,
                })
            })
            .collect()
    }

    /// Jumlah task aktif (sedang dikerjakan) per run, lintas semua scheduler; dasar keadilan antar run.
    pub async fn active_counts(&self) -> Result<std::collections::HashMap<Uuid, i64>, StoreError> {
        let rows = sqlx::query(
            "SELECT project_run_id,count(*) AS active FROM tasks
             WHERE status IN ('ASSIGNED','RUNNING','SELF_CHECK','REVIEW','VERIFY','INTEGRATE')
             GROUP BY project_run_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        Ok(rows
            .iter()
            .map(|row| (row.get("project_run_id"), row.get("active")))
            .collect())
    }

    /// Perpanjang kepemilikan claim. `Conflict::Claim` berarti claim sudah hilang (dipulihkan scheduler
    /// lain, selesai, atau milik owner lain): pemegang lama harus berhenti dan tidak boleh menulis hasil.
    pub async fn heartbeat(&self, attempt_id: Uuid, owner: Uuid) -> Result<(), StoreError> {
        let renewed = sqlx::query(
            "UPDATE agent_runs SET heartbeat_at=now()
             WHERE id=$1 AND dispatch_owner=$2 AND finished_at IS NULL AND status IN ('assigned','running')",
        )
        .bind(attempt_id)
        .bind(owner)
        .execute(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        if renewed.rows_affected() == 1 {
            Ok(())
        } else {
            Err(StoreError::Conflict(Conflict::Claim))
        }
    }

    /// Claim aktif yang heartbeat terakhirnya (atau waktu mulai bila belum ada) lebih tua dari
    /// `stale_after_seconds`. Hanya membaca; pemulihan dilakukan `recover_abandoned`.
    pub async fn stale_claims(
        &self,
        stale_after_seconds: i64,
        limit: i64,
    ) -> Result<Vec<StaleClaim>, StoreError> {
        if !(1..=86_400).contains(&stale_after_seconds) || !(1..=100).contains(&limit) {
            return Err(StoreError::InvalidId("stale_after_seconds"));
        }
        let rows = sqlx::query(
            "SELECT id,task_id,dispatch_owner FROM agent_runs
             WHERE finished_at IS NULL AND status IN ('assigned','running')
               AND COALESCE(heartbeat_at,started_at) < now() - ($1 * interval '1 second')
             ORDER BY COALESCE(heartbeat_at,started_at),id LIMIT $2",
        )
        .bind(stale_after_seconds)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        Ok(rows
            .iter()
            .map(|row| StaleClaim {
                attempt_id: row.get("id"),
                task_id: row.get("task_id"),
                owner: row.get("dispatch_owner"),
            })
            .collect())
    }

    /// Pulihkan claim stale lewat jalur recovery yang sama dengan startup recovery (tidak mengulang
    /// side effect yang ambigu). Aman dipanggil bersamaan oleh banyak scheduler: yang kalah balapan
    /// menerima NotFound dan dilewati. Mengembalikan pemulihan yang benar-benar dilakukan instance ini.
    pub async fn recover_abandoned(
        &self,
        stale_after_seconds: i64,
        limit: i64,
    ) -> Result<Vec<(StaleClaim, RecoveryDisposition)>, StoreError> {
        let mut recovered = Vec::new();
        for claim in self.stale_claims(stale_after_seconds, limit).await? {
            match self
                .tasks
                .recover_stale_attempt(claim.attempt_id, false)
                .await
            {
                Ok(result) => recovered.push((claim, result.disposition)),
                Err(StoreError::NotFound) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(recovered)
    }
}
