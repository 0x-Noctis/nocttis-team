//! Recovery saat startup (M4-008): pulihkan attempt yang ditinggalkan, lepas lease dan reservasi budget
//! miliknya, dan catat event `recovery` per attempt.
//!
//! Prinsip keamanan: sebuah task hanya dijalankan ulang (attempt baru dari worktree bersih) bila TIDAK ADA
//! kemungkinan efek samping yang hasilnya tidak diketahui. Tool call yang masih `in_progress`, tahap
//! INTEGRATE, atau worktree yang tidak bisa diverifikasi berarti efek samping ambigu -> NEEDS_HUMAN.
//! Tool call yang sudah `completed` adalah bukti idempotensi (hasilnya tercatat), dan worktree lama
//! dibuang sehingga attempt baru tidak mewarisi setengah pekerjaan.

use std::path::PathBuf;

use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    domain::task::TaskStatus,
    runner::git::GitWorktreeManager,
    store::{
        budget::{BudgetError, BudgetStore},
        event::{AttemptStatus, AttemptUpdate, RecoveryDisposition},
        lease::{LeaseError, LeaseStore},
        scheduler::{SchedulerStore, StaleClaim},
        task::{StoreError, TaskRepository},
    },
};

const PAGE: i64 = 100;
const MAX_PAGES: usize = 50;

pub struct RecoveryConfig {
    pub worktree_root: PathBuf,
    /// Attempt tanpa heartbeat selama ini dianggap ditinggalkan. Attempt yang lebih segar dibiarkan
    /// (bisa milik instance lain yang masih hidup) dan dipulihkan pada sapuan berikutnya.
    pub stale_after_seconds: i64,
}

#[derive(Debug)]
pub enum RecoveryError {
    Store(StoreError),
    Lease(LeaseError),
    Budget(BudgetError),
    Database(sqlx::Error),
}

impl std::fmt::Display for RecoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "recovery store error: {error}"),
            Self::Lease(error) => write!(formatter, "recovery lease error: {error}"),
            Self::Budget(error) => write!(formatter, "recovery budget error: {error}"),
            Self::Database(_) => formatter.write_str("recovery database error"),
        }
    }
}

impl std::error::Error for RecoveryError {}

macro_rules! from_error {
    ($($source:ty => $variant:ident),*) => {$(
        impl From<$source> for RecoveryError { fn from(error: $source) -> Self { Self::$variant(error) } }
    )*};
}
from_error!(StoreError => Store, LeaseError => Lease, BudgetError => Budget, sqlx::Error => Database);

/// Keadaan worktree attempt yang ditinggalkan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreeState {
    /// Ditemukan, valid, dan sudah dibuang.
    Cleaned,
    /// Tidak pernah dibuat atau sudah hilang.
    Missing,
    /// Ada tetapi tidak bisa divalidasi atau dibuang (cabang/base tak cocok, Git gagal).
    Unverifiable,
}

impl WorktreeState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cleaned => "cleaned",
            Self::Missing => "missing",
            Self::Unverifiable => "unverifiable",
        }
    }
}

/// Apakah worktree ini memaksa penyerahan ke manusia? Task ASSIGNED belum menjalankan apa pun, jadi
/// worktree yang belum ada adalah keadaan normal setelah crash tepat sesudah claim. Pada tahap lain,
/// worktree yang hilang atau tak terverifikasi berarti pekerjaan tak bisa dipastikan.
pub fn requires_human(task: TaskStatus, worktree: WorktreeState) -> bool {
    match worktree {
        WorktreeState::Cleaned => false,
        WorktreeState::Missing => task != TaskStatus::Assigned,
        WorktreeState::Unverifiable => true,
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Task yang dikembalikan ke READY untuk attempt baru.
    pub requeued: Vec<String>,
    /// Task yang diserahkan ke manusia beserta alasannya (kode error attempt).
    pub needs_human: Vec<(String, String)>,
    /// Attempt yang ditutup tanpa mengubah task karena task sudah selesai/dibatalkan.
    pub closed: Vec<String>,
    pub leases_released: u64,
    pub dead_leases_swept: u64,
    pub reservations_released: u64,
    pub events: usize,
}

/// Kunci advisory sesi yang menyerialkan sapuan recovery di seluruh instance.
pub const RECOVERY_LOCK: i64 = 0x004e_4f43_5452_4543; // "NOCTREC"

/// Jalankan satu sapuan recovery. Aman dipanggil berulang dan oleh beberapa instance sekaligus: sapuan
/// diserialkan dengan advisory lock sesi, dan pemanggil yang kalah mengembalikan laporan kosong karena
/// instance lain sedang memulihkan. Serialisasi ini perlu (bukan sekadar optimasi): inspeksi worktree
/// membuangnya, sehingga pemulih kedua akan melihatnya "hilang" dan salah menyerahkan task aman ke manusia.
pub async fn recover(
    pool: &PgPool,
    config: &RecoveryConfig,
) -> Result<RecoveryReport, RecoveryError> {
    let mut connection = pool.acquire().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(RECOVERY_LOCK)
        .fetch_one(&mut *connection)
        .await?;
    if !locked {
        return Ok(RecoveryReport::default());
    }
    let result = sweep(pool, config).await;
    // Kunci sesi harus lepas sebelum koneksi kembali ke pool; bila unlock gagal, putuskan koneksinya
    // (lock hilang bersama sesi) daripada mengembalikannya ke pool dalam keadaan terkunci.
    if sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(RECOVERY_LOCK)
        .execute(&mut *connection)
        .await
        .is_err()
    {
        drop(connection.detach());
    }
    result
}

async fn sweep(pool: &PgPool, config: &RecoveryConfig) -> Result<RecoveryReport, RecoveryError> {
    let store = SchedulerStore::new(pool.clone());
    let tasks = TaskRepository::new(pool.clone());
    let leases = LeaseStore::new(pool.clone());
    let budgets = BudgetStore::new(pool.clone());
    let mut report = RecoveryReport::default();

    for _ in 0..MAX_PAGES {
        let stale = store.stale_claims(config.stale_after_seconds, PAGE).await?;
        if stale.is_empty() {
            break;
        }
        let mut progressed = false;
        for claim in &stale {
            progressed |=
                recover_one(pool, &tasks, &leases, &budgets, config, claim, &mut report).await?;
        }
        // Kalau tak satu pun berhasil (semua kalah balapan), hentikan supaya tidak berputar.
        if !progressed {
            break;
        }
    }

    // Lease yang tidak diperpanjang dalam jendela stale tidak mungkin dimiliki pemegang yang masih hidup
    // (slot memperbarui tiap heartbeat); lease kedaluwarsa juga dibuang.
    report.dead_leases_swept = sqlx::query(
        "DELETE FROM file_leases WHERE expires_at <= now() OR renewed_at < now() - ($1 * interval '1 second')",
    )
    .bind(config.stale_after_seconds)
    .execute(pool)
    .await?
    .rows_affected();
    // Reservasi yang tertahan untuk attempt yang sudah selesai tidak mungkin punya request yang masih berjalan.
    report.reservations_released += sqlx::query(
        "UPDATE budget_reservations SET status='released',closed_at=now()
         WHERE status='held' AND agent_run_id IN (SELECT id FROM agent_runs WHERE finished_at IS NOT NULL)",
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(report)
}

/// Pulihkan satu attempt. True bila instance ini yang memulihkannya.
async fn recover_one(
    pool: &PgPool,
    tasks: &TaskRepository,
    leases: &LeaseStore,
    budgets: &BudgetStore,
    config: &RecoveryConfig,
    claim: &StaleClaim,
    report: &mut RecoveryReport,
) -> Result<bool, RecoveryError> {
    let attempt = match tasks.get_attempt(claim.attempt_id).await {
        Ok(attempt) => attempt,
        Err(StoreError::NotFound) => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let task = tasks.get(&claim.task_id).await?;
    let previous = task.status;

    // Task yang sudah tidak bisa dipulihkan lewat state machine (selesai, dibatalkan, menunggu manusia, ...):
    // cukup tutup attempt-nya supaya tidak dianggap hidup selamanya.
    let recoverable = matches!(
        previous,
        TaskStatus::Assigned
            | TaskStatus::Running
            | TaskStatus::SelfCheck
            | TaskStatus::Review
            | TaskStatus::Verify
            | TaskStatus::Integrate
    );
    let (disposition, worktree) = if recoverable {
        let worktree = inspect_worktree(
            tasks,
            config,
            &claim.task_id,
            &attempt.branch,
            &attempt.base_commit,
        )
        .await;
        match tasks
            .recover_stale_attempt(claim.attempt_id, requires_human(previous, worktree))
            .await
        {
            Ok(result) => (Some(result.disposition), worktree),
            Err(StoreError::NotFound) => return Ok(false),
            Err(error) => return Err(error.into()),
        }
    } else {
        match tasks
            .update_attempt(
                claim.attempt_id,
                &AttemptUpdate {
                    status: AttemptStatus::Failed,
                    error_code: Some("recovery.task_closed".to_owned()),
                },
            )
            .await
        {
            Ok(_) => (None, WorktreeState::Missing),
            Err(StoreError::NotFound) => return Ok(false),
            Err(error) => return Err(error.into()),
        }
    };

    let leases_released = leases.release_task(&claim.task_id).await?;
    let held = budgets.release_attempt(claim.attempt_id).await?;
    report.leases_released += leases_released;
    report.reservations_released += held;

    let code: Option<String> = sqlx::query_scalar("SELECT error_code FROM agent_runs WHERE id=$1")
        .bind(claim.attempt_id)
        .fetch_one(pool)
        .await?;
    let (label, task_id) = match disposition {
        Some(RecoveryDisposition::Requeued) => {
            report.requeued.push(claim.task_id.clone());
            ("requeued", claim.task_id.clone())
        }
        Some(RecoveryDisposition::RecoveryRequired) => {
            report
                .needs_human
                .push((claim.task_id.clone(), code.clone().unwrap_or_default()));
            ("recovery_required", claim.task_id.clone())
        }
        None => {
            report.closed.push(claim.task_id.clone());
            ("closed", claim.task_id.clone())
        }
    };
    record_event(
        pool,
        &task_id,
        claim.attempt_id,
        &json!({
            "attempt_id": claim.attempt_id,
            "attempt": attempt.attempt,
            "previous_status": previous,
            "disposition": label,
            "reason": code,
            "worktree": worktree.as_str(),
            "leases_released": leases_released,
        }),
    )
    .await?;
    report.events += 1;
    Ok(true)
}

/// Periksa dan buang worktree attempt. Kegagalan apa pun dianggap tak terverifikasi.
async fn inspect_worktree(
    tasks: &TaskRepository,
    config: &RecoveryConfig,
    task_id: &str,
    branch: &str,
    base_commit: &str,
) -> WorktreeState {
    let Ok(repository) = tasks.project_repository(task_id).await else {
        return WorktreeState::Unverifiable;
    };
    let Ok(git) = GitWorktreeManager::new(repository, &config.worktree_root) else {
        return WorktreeState::Unverifiable;
    };
    if !git.worktree_root().join(task_id).exists() {
        return WorktreeState::Missing;
    }
    match git.open(task_id, branch, base_commit) {
        Ok(worktree) if git.cleanup(&worktree).is_ok() => WorktreeState::Cleaned,
        _ => WorktreeState::Unverifiable,
    }
}

/// Event `recovery` per attempt; kunci operasi membuatnya idempoten bila sapuan berulang.
async fn record_event(
    pool: &PgPool,
    task_id: &str,
    attempt_id: Uuid,
    payload: &serde_json::Value,
) -> Result<(), RecoveryError> {
    sqlx::query(
        "INSERT INTO events (project_run_id,task_id,actor_type,event_type,payload,operation_key)
         SELECT t.project_run_id,t.id,'system','recovery',$2::jsonb,$3 FROM tasks t WHERE t.id=$1
         ON CONFLICT (task_id,operation_key) WHERE operation_key IS NOT NULL DO NOTHING",
    )
    .bind(task_id)
    .bind(payload)
    .bind(format!("recovery-{attempt_id}"))
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_missing_is_normal_only_before_work_started() {
        use TaskStatus::*;
        use WorktreeState::*;
        for (task, worktree, human) in [
            (Assigned, Cleaned, false),
            (Assigned, Missing, false),
            (Assigned, Unverifiable, true),
            (Running, Cleaned, false),
            (Running, Missing, true),
            (Running, Unverifiable, true),
            (Integrate, Missing, true),
            (Review, Cleaned, false),
        ] {
            assert_eq!(
                requires_human(task, worktree),
                human,
                "{task:?} {worktree:?}"
            );
        }
    }
}
