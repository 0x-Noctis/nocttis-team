//! Scheduler paralel (M4-005): 1-4 slot worker di atas claim atomik (M4-001), file lease (M4-002), dan
//! budget guard (M4-003).
//!
//! Satu `step` mengisi slot yang kosong: tiap kandidat (task READY yang lolos syarat dependency, budget
//! reservasi, dan status run) lebih dulu memegang file lease-nya, baru diklaim spesifik. Dengan urutan itu
//! bentrok lease tidak pernah mengubah state task (tidak ada ASSIGNED -> READY bolak-balik), dan kegagalan
//! di tengah jalan cukup melepas lease. Eksekusi sebenarnya didelegasikan ke `SlotRunner`, sehingga
//! scheduler dapat diuji tanpa model, Git, atau Docker.

use std::{
    collections::HashMap,
    future::Future,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use sqlx::{PgPool, Row};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, watch};
use uuid::Uuid;

use crate::{
    domain::{budget::BudgetLevel, state_machine::Actor, task::TaskStatus},
    store::{
        budget::{BudgetError, BudgetStore},
        event::{AttemptStatus, AttemptUpdate},
        lease::{LeaseError, LeaseStore},
        scheduler::{Candidate, ClaimRequest, ClaimedTask, SchedulerStore},
        task::{Conflict, StoreError, TaskRepository},
    },
};

pub const MAX_SLOTS: usize = 4;

/// Mengapa sebuah slot diminta berhenti. Runner harus berhenti di titik aman berikutnya.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// Run dibatalkan: task akan ditandai CANCELLED.
    Cancel,
    /// Scheduler dimatikan dan masa tenggang habis: attempt dibiarkan untuk recovery berikutnya.
    Shutdown,
    /// Claim sudah diambil alih (mis. dipulihkan scheduler lain): jangan menulis hasil apa pun.
    Lost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    /// Runner menyelesaikan pekerjaannya dan mengurus status akhir task sendiri.
    Finished,
    /// Runner berhenti karena `SlotControl` meminta.
    Stopped,
}

/// Penerima sinyal berhenti untuk satu slot.
#[derive(Clone)]
pub struct SlotControl {
    receiver: watch::Receiver<Option<StopReason>>,
}

impl SlotControl {
    pub fn stop_requested(&self) -> Option<StopReason> {
        *self.receiver.borrow()
    }

    /// Menunggu sampai ada permintaan berhenti.
    pub async fn stopped(&mut self) -> StopReason {
        loop {
            if let Some(reason) = *self.receiver.borrow_and_update() {
                return reason;
            }
            if self.receiver.changed().await.is_err() {
                return StopReason::Shutdown;
            }
        }
    }
}

/// Menjalankan satu task yang sudah diklaim (worker -> review -> verify di produksi).
pub trait SlotRunner: Send + Sync + 'static {
    fn run(
        &self,
        task: &ClaimedTask,
        control: SlotControl,
    ) -> impl Future<Output = RunOutcome> + Send;
}

#[derive(Clone, Debug)]
pub struct ParallelConfig {
    /// Jumlah slot worker, 1..=MAX_SLOTS.
    pub slots: usize,
    pub provider_id: String,
    pub model_id: String,
    pub retention_seconds: i64,
    pub lease_ttl_seconds: i64,
    pub heartbeat_interval: Duration,
    /// Claim tanpa heartbeat selama ini dianggap ditinggalkan dan dipulihkan.
    pub stale_after_seconds: i64,
    /// Jeda polling saat tidak ada pekerjaan: mulai dari `idle_min`, berlipat dua sampai `idle_max`.
    pub idle_min: Duration,
    pub idle_max: Duration,
    /// Setelah shutdown, slot yang masih berjalan diberi waktu ini sebelum diminta berhenti.
    pub shutdown_grace: Duration,
    /// Berapa kandidat yang dilihat per langkah (membatasi kerja saat banyak task terblokir).
    pub candidate_window: i64,
    pub recover_every: Duration,
}

impl ParallelConfig {
    fn validate(&self) -> Result<(), SchedulerError> {
        let bad = |field| Err(SchedulerError::Config(field));
        if !(1..=MAX_SLOTS).contains(&self.slots) {
            return bad("slots");
        }
        if self.heartbeat_interval.is_zero()
            || self.idle_min.is_zero()
            || self.idle_max < self.idle_min
        {
            return bad("intervals");
        }
        // Lease harus bertahan beberapa heartbeat; stale tidak boleh lebih pendek dari dua heartbeat.
        if self.lease_ttl_seconds < 1
            || Duration::from_secs(self.lease_ttl_seconds as u64) < self.heartbeat_interval * 3
        {
            return bad("lease_ttl_seconds");
        }
        if self.stale_after_seconds < 1
            || Duration::from_secs(self.stale_after_seconds as u64) < self.heartbeat_interval * 2
        {
            return bad("stale_after_seconds");
        }
        if self.candidate_window < 1 || self.candidate_window > 200 || self.retention_seconds < 1 {
            return bad("candidate_window");
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum SchedulerError {
    Config(&'static str),
    Store(StoreError),
    Lease(LeaseError),
    Budget(BudgetError),
    Database(sqlx::Error),
}

impl std::fmt::Display for SchedulerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(field) => write!(formatter, "invalid scheduler config: {field}"),
            Self::Store(error) => write!(formatter, "scheduler store error: {error}"),
            Self::Lease(error) => write!(formatter, "scheduler lease error: {error}"),
            Self::Budget(error) => write!(formatter, "scheduler budget error: {error}"),
            Self::Database(_) => formatter.write_str("scheduler database error"),
        }
    }
}

impl std::error::Error for SchedulerError {}

macro_rules! from_error {
    ($($source:ty => $variant:ident),*) => {$(
        impl From<$source> for SchedulerError { fn from(error: $source) -> Self { Self::$variant(error) } }
    )*};
}
from_error!(StoreError => Store, LeaseError => Lease, BudgetError => Budget, sqlx::Error => Database);

/// Alasan sebuah kandidat dilewati pada langkah ini; kandidat tetap READY dan dicoba lagi nanti.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Skip {
    /// Scope file bentrok dengan lease task lain.
    Lease { holder_task_id: String },
    /// Budget run sudah di batas untuk pekerjaan biasa (level Stop).
    Budget,
    /// Scope task tidak valid sehingga tidak bisa dikunci.
    InvalidScope,
    /// Task diambil scheduler lain atau berubah status di antara daftar kandidat dan claim.
    Raced,
}

#[derive(Debug, Default)]
pub struct StepReport {
    pub started: Vec<String>,
    pub skipped: Vec<(String, Skip)>,
}

type BaseCommit = Arc<dyn Fn(&Path) -> Option<String> + Send + Sync>;

pub struct ParallelScheduler<R: SlotRunner> {
    pool: PgPool,
    store: SchedulerStore,
    leases: LeaseStore,
    budgets: BudgetStore,
    tasks: TaskRepository,
    runner: R,
    config: ParallelConfig,
    base_commit: BaseCommit,
    permits: Arc<Semaphore>,
    wake: Notify,
    shutdown: watch::Sender<bool>,
}

/// Pilih indeks kandidat berikutnya: run dengan task aktif paling sedikit lebih dulu (keadilan antar
/// run), lalu urutan asli (priority, umur, id). Satu run dengan banyak task berprioritas tinggi tidak
/// bisa memonopoli semua slot selama run lain punya pekerjaan siap.
pub fn pick(candidates: &[Candidate], active: &HashMap<Uuid, i64>) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .min_by_key(|(index, candidate)| {
            (active.get(&candidate.run_id).copied().unwrap_or(0), *index)
        })
        .map(|(index, _)| index)
}

impl<R: SlotRunner> ParallelScheduler<R> {
    pub fn new(
        pool: PgPool,
        runner: R,
        config: ParallelConfig,
        base_commit: impl Fn(&Path) -> Option<String> + Send + Sync + 'static,
    ) -> Result<Arc<Self>, SchedulerError> {
        config.validate()?;
        Ok(Arc::new(Self {
            store: SchedulerStore::new(pool.clone()),
            leases: LeaseStore::new(pool.clone()),
            budgets: BudgetStore::new(pool.clone()),
            tasks: TaskRepository::new(pool.clone()),
            permits: Arc::new(Semaphore::new(config.slots)),
            wake: Notify::new(),
            shutdown: watch::channel(false).0,
            pool,
            runner,
            config,
            base_commit: Arc::new(base_commit),
        }))
    }

    /// Bangunkan loop `run` segera (mis. setelah plan disetujui atau run di-resume).
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    /// Berhenti mengambil task baru; `run` selesai setelah slot yang berjalan habis atau masa tenggang lewat.
    pub fn shutdown(&self) {
        self.shutdown.send_replace(true);
        self.wake.notify_one();
    }

    pub fn free_slots(&self) -> usize {
        self.permits.available_permits()
    }

    pub fn busy_slots(&self) -> usize {
        self.config.slots - self.free_slots()
    }

    /// Tunggu sampai semua slot kosong (untuk pengujian dan drain).
    pub async fn wait_idle(&self) {
        let all = u32::try_from(self.config.slots).unwrap_or(u32::MAX);
        let _ = self.permits.acquire_many(all).await;
    }

    /// Isi slot kosong dengan kandidat yang layak. Dipanggil `run` secara berkala dan oleh pengujian.
    pub async fn step(self: &Arc<Self>) -> Result<StepReport, SchedulerError> {
        let mut report = StepReport::default();
        // Shutdown tidak boleh mengambil task baru; slot penuh = backpressure tanpa menyentuh database.
        if *self.shutdown.borrow() || self.free_slots() == 0 {
            return Ok(report);
        }
        let mut candidates = self
            .store
            .ready_candidates(self.config.candidate_window)
            .await?;
        if candidates.is_empty() {
            return Ok(report);
        }
        let mut active = self.store.active_counts().await?;
        let mut run_stopped: HashMap<Uuid, bool> = HashMap::new();
        while let Ok(permit) = self.permits.clone().try_acquire_owned() {
            if !self
                .start_next(
                    &mut candidates,
                    &mut active,
                    &mut run_stopped,
                    &mut report,
                    permit,
                )
                .await?
            {
                break;
            }
        }
        Ok(report)
    }

    /// Coba memulai kandidat berikutnya; false bila tidak ada lagi yang bisa dimulai (permit dilepas).
    async fn start_next(
        self: &Arc<Self>,
        candidates: &mut Vec<Candidate>,
        active: &mut HashMap<Uuid, i64>,
        run_stopped: &mut HashMap<Uuid, bool>,
        report: &mut StepReport,
        permit: OwnedSemaphorePermit,
    ) -> Result<bool, SchedulerError> {
        while let Some(index) = pick(candidates, active) {
            let candidate = candidates.remove(index);
            if self.run_is_stopped(candidate.run_id, run_stopped).await? {
                report.skipped.push((candidate.task_id, Skip::Budget));
                continue;
            }
            let owner = Uuid::new_v4();
            let scopes: Vec<&str> = candidate.allowed_paths.iter().map(String::as_str).collect();
            match self
                .leases
                .acquire(
                    candidate.run_id,
                    &candidate.task_id,
                    owner,
                    &scopes,
                    self.config.lease_ttl_seconds,
                )
                .await
            {
                Ok(_) => {}
                Err(LeaseError::Conflict { holder_task_id, .. }) => {
                    report
                        .skipped
                        .push((candidate.task_id, Skip::Lease { holder_task_id }));
                    continue;
                }
                Err(LeaseError::InvalidScope(_) | LeaseError::InvalidInput("patterns")) => {
                    report.skipped.push((candidate.task_id, Skip::InvalidScope));
                    continue;
                }
                Err(error) => return Err(error.into()),
            }
            let request = ClaimRequest {
                owner,
                provider_id: &self.config.provider_id,
                model_id: &self.config.model_id,
                retention_seconds: self.config.retention_seconds,
                only_task: Some(&candidate.task_id),
            };
            match self
                .store
                .claim_next(&request, |path| (self.base_commit)(path))
                .await
            {
                Ok(Some(claimed)) => {
                    *active.entry(candidate.run_id).or_default() += 1;
                    report.started.push(candidate.task_id);
                    self.spawn_slot(claimed, owner, permit);
                    return Ok(true);
                }
                Ok(None) => {
                    self.release_lease(candidate.run_id, owner).await;
                    report.skipped.push((candidate.task_id, Skip::Raced));
                }
                Err(error) => {
                    self.release_lease(candidate.run_id, owner).await;
                    return Err(error.into());
                }
            }
        }
        Ok(false)
    }

    async fn run_is_stopped(
        &self,
        run_id: Uuid,
        cache: &mut HashMap<Uuid, bool>,
    ) -> Result<bool, SchedulerError> {
        if let Some(stopped) = cache.get(&run_id) {
            return Ok(*stopped);
        }
        let stopped = self.budgets.run_view(run_id).await?.level == BudgetLevel::Stop;
        cache.insert(run_id, stopped);
        Ok(stopped)
    }

    async fn release_lease(&self, run_id: Uuid, owner: Uuid) {
        if let Err(error) = self.leases.release(run_id, owner).await {
            tracing::error!(%error, "gagal melepas file lease");
        }
    }

    fn spawn_slot(
        self: &Arc<Self>,
        claimed: ClaimedTask,
        owner: Uuid,
        permit: OwnedSemaphorePermit,
    ) {
        let this = self.clone();
        tokio::spawn(async move {
            this.run_slot(claimed, owner).await;
            // Permit dilepas setelah semua pembersihan selesai, lalu loop dibangunkan agar slot cepat terisi.
            drop(permit);
            this.wake.notify_one();
        });
    }

    async fn run_slot(self: &Arc<Self>, claimed: ClaimedTask, owner: Uuid) {
        let (sender, receiver) = watch::channel(None);
        let sender = Arc::new(sender);
        let beat = tokio::spawn(self.clone().heartbeat_loop(
            claimed.attempt.id,
            claimed.run_id,
            owner,
            sender.clone(),
        ));
        let outcome = self.runner.run(&claimed, SlotControl { receiver }).await;
        beat.abort();
        let reason = *sender.borrow();
        self.release_lease(claimed.run_id, owner).await;
        // Reservasi request yang masih menggantung tidak boleh menahan budget setelah slot selesai.
        if let Err(error) = self.budgets.release_attempt(claimed.attempt.id).await {
            tracing::error!(%error, "gagal melepas reservasi budget");
        }
        if outcome == RunOutcome::Stopped
            && reason == Some(StopReason::Cancel)
            && let Err(error) = self.finalize_cancelled(&claimed).await
        {
            tracing::error!(%error, "gagal memfinalisasi task yang dibatalkan");
        }
        // Lost: pemulih lain memegang attempt ini. Shutdown: attempt dibiarkan agar recovery berikutnya memulihkannya.
    }

    /// Tandai attempt gagal dan task CANCELLED (kecuali sudah terminal).
    async fn finalize_cancelled(&self, claimed: &ClaimedTask) -> Result<(), SchedulerError> {
        self.tasks
            .update_attempt(
                claimed.attempt.id,
                &AttemptUpdate {
                    status: AttemptStatus::Failed,
                    error_code: Some("scheduler.cancelled".to_owned()),
                },
            )
            .await?;
        let task = self.tasks.get(claimed.attempt.task_id.as_str()).await?;
        if !task.status.is_terminal() {
            match self
                .tasks
                .transition(
                    claimed.attempt.task_id.as_str(),
                    task.version,
                    TaskStatus::Cancelled,
                    Actor::System,
                )
                .await
            {
                // Versi berubah = task bergerak di antara baca dan tulis; biarkan pemilik barunya memutuskan.
                Ok(_) | Err(StoreError::Conflict(Conflict::StaleVersion)) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    /// Selama runner berjalan: perbarui heartbeat dan lease, deteksi pembatalan run dan kehilangan claim,
    /// dan terapkan masa tenggang shutdown. Berakhir saat claim hilang atau di-abort oleh `run_slot`.
    async fn heartbeat_loop(
        self: Arc<Self>,
        attempt_id: Uuid,
        run_id: Uuid,
        owner: Uuid,
        stop: Arc<watch::Sender<Option<StopReason>>>,
    ) {
        let mut shutdown = self.shutdown.subscribe();
        let mut shutdown_seen: Option<Instant> = None;
        loop {
            let mut wait = self.config.heartbeat_interval;
            if let Some(seen) = shutdown_seen {
                wait = wait.min(
                    self.config
                        .shutdown_grace
                        .saturating_sub(seen.elapsed())
                        .max(Duration::from_millis(1)),
                );
            }
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                _ = shutdown.changed() => {}
            }
            if *shutdown.borrow() && shutdown_seen.is_none() {
                shutdown_seen = Some(Instant::now());
            }
            match self.store.heartbeat(attempt_id, owner).await {
                Ok(()) => {}
                Err(StoreError::Conflict(Conflict::Claim)) => {
                    stop.send_replace(Some(StopReason::Lost));
                    return;
                }
                Err(error) => tracing::warn!(%error, "heartbeat gagal; dicoba lagi"),
            }
            match self
                .leases
                .renew(run_id, owner, self.config.lease_ttl_seconds)
                .await
            {
                Ok(_) => {}
                Err(LeaseError::Lost) => {
                    stop.send_replace(Some(StopReason::Lost));
                    return;
                }
                Err(error) => tracing::warn!(%error, "perpanjangan lease gagal; dicoba lagi"),
            }
            if let Ok(row) = sqlx::query("SELECT status FROM project_runs WHERE id=$1")
                .bind(run_id)
                .fetch_one(&self.pool)
                .await
                && row.get::<&str, _>("status") == "CANCELLED"
                && stop.borrow().is_none()
            {
                stop.send_replace(Some(StopReason::Cancel));
            }
            if shutdown_seen.is_some_and(|seen| seen.elapsed() >= self.config.shutdown_grace)
                && stop.borrow().is_none()
            {
                stop.send_replace(Some(StopReason::Shutdown));
            }
        }
    }

    /// Pulihkan claim yang ditinggalkan: attempt stale dikembalikan lewat jalur recovery, dan lease serta
    /// reservasi budget miliknya dilepas supaya attempt pengganti tidak menunggu kedaluwarsa.
    pub async fn recover(&self) -> Result<usize, SchedulerError> {
        let recovered = self
            .store
            .recover_abandoned(self.config.stale_after_seconds, 20)
            .await?;
        for (claim, _) in &recovered {
            self.leases.release_task(&claim.task_id).await?;
            self.budgets.release_attempt(claim.attempt_id).await?;
        }
        Ok(recovered.len())
    }

    /// Tandai `DONE` run yang berstatus `RUNNING` dan seluruh task-nya sudah `DONE`. Sebelumnya tidak ada kode yang
    /// menutup run, jadi run tetap `RUNNING` selamanya di UI dan API walau pekerjaannya selesai. Run kosong (belum ada
    /// task), run dijeda, dan run dengan task yang belum `DONE` tidak disentuh. Idempoten.
    pub async fn complete_finished_runs(&self) -> Result<u64, SchedulerError> {
        let done = sqlx::query(
            "UPDATE project_runs r SET status='DONE',updated_at=now()
             WHERE r.status='RUNNING'
               AND EXISTS (SELECT 1 FROM tasks t WHERE t.project_run_id=r.id)
               AND NOT EXISTS (SELECT 1 FROM tasks t WHERE t.project_run_id=r.id AND t.status<>'DONE')",
        )
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected())
    }

    /// Loop utama sampai `shutdown()`: isi slot, pulihkan claim stale, tidur dengan backoff (atau sampai
    /// `wake()`), lalu menguras slot yang masih berjalan.
    pub async fn run(self: Arc<Self>) {
        let mut shutdown = self.shutdown.subscribe();
        let mut idle = self.config.idle_min;
        let mut last_recover: Option<Instant> = None;
        while !*shutdown.borrow() {
            if last_recover.is_none_or(|at| at.elapsed() >= self.config.recover_every) {
                if let Err(error) = self.recover().await {
                    tracing::error!(%error, "recovery claim stale gagal");
                }
                last_recover = Some(Instant::now());
            }
            if let Err(error) = self.complete_finished_runs().await {
                tracing::error!(%error, "penutupan run selesai gagal");
            }
            let started = match self.step().await {
                Ok(report) => report.started.len(),
                Err(error) => {
                    tracing::error!(%error, "langkah scheduler gagal");
                    0
                }
            };
            idle = if started > 0 {
                self.config.idle_min
            } else {
                (idle * 2).min(self.config.idle_max)
            };
            tokio::select! {
                () = self.wake.notified() => idle = self.config.idle_min,
                () = tokio::time::sleep(idle) => {}
                _ = shutdown.changed() => {}
            }
        }
        // Drain: slot yang berjalan diberi masa tenggang, lalu diminta berhenti; batas keras mencegah tergantung.
        let limit = self.config.shutdown_grace * 2 + Duration::from_secs(5);
        if tokio::time::timeout(limit, self.wait_idle()).await.is_err() {
            tracing::warn!("slot belum berhenti setelah masa tenggang shutdown");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(task: &str, run: Uuid) -> Candidate {
        Candidate {
            task_id: task.to_owned(),
            run_id: run,
            priority: 0,
            allowed_paths: vec![],
        }
    }

    #[test]
    fn pick_prefers_least_loaded_run_then_original_order() {
        let (busy, idle) = (Uuid::new_v4(), Uuid::new_v4());
        let candidates = vec![
            candidate("a", busy),
            candidate("b", busy),
            candidate("c", idle),
            candidate("d", idle),
        ];
        let mut active = HashMap::from([(busy, 2)]);
        assert_eq!(
            pick(&candidates, &active),
            Some(2),
            "run yang sepi lebih dulu"
        );
        active.insert(idle, 2);
        assert_eq!(
            pick(&candidates, &active),
            Some(0),
            "beban sama: urutan priority asli"
        );
        active.insert(idle, 1);
        assert_eq!(pick(&candidates, &active), Some(2));
        assert_eq!(pick(&[], &active), None);
    }
}
